pub mod captioner;
pub mod models;
pub mod utils;

use anyhow::anyhow;
use aws_config::meta::region::RegionProviderChain;
use aws_config::BehaviorVersion;
use aws_types::region::Region;
use core::panic;
use serde::ser::Error;
use serde::Deserialize;
use serde_json::Value;
use std::{env, io};

use anyhow::Result;

use aws_sdk_bedrockruntime::primitives::Blob;
use aws_sdk_bedrockruntime::types::ResponseStream;

use models::claudev3::{ClaudeImageSource, ClaudeV3Body, ClaudeV3Response};

use models::check_for_streaming;
use models::load_config;




use std::io::Write;

use crate::captioner::Image;

//======================================== AWS
pub async fn configure_aws(s: String) -> aws_config::SdkConfig {
    let provider =
        RegionProviderChain::first_try(env::var("AWS_DEFAULT_REGION").ok().map(Region::new))
            .or_default_provider()
            .or_else(Region::new(s));

    aws_config::defaults(BehaviorVersion::latest())
        .region(provider)
        .load()
        .await
}
//======================================== END AWS

#[derive(Debug)]
pub enum RunType {
    Standard,
    Captioning,
}

#[derive(Debug)]
struct BedrockCall {
    pub body: Blob,
    pub content_type: String,
    pub accept: String,
    pub model_id: String,
}

impl BedrockCall {
    fn new(body: Blob, content_type: String, accept: String, model_id: String) -> BedrockCall {
        BedrockCall {
            body,
            content_type,
            accept,
            model_id,
        }
    }
}

// Eventually this wil need to support every model in ArgModels, but
// this will not necessarily be a 1-to-1 mapping. For example, minor
// version updates to the model will have the same body, but differnet
// values than in ArgModels. Thus, |ArgModels| >= |BedrockCallSum|.
enum BedrockCallSum {
    Claude3BCS {
        model_id: String,
        body: ClaudeV3Body,
    },

}

// Using a sum type to represent all models that can go through here.
// This way if each model needs special processing to make a BedrockCall
// that can be implemented in one place.
fn bcs_to_bedrock_call(bcs: BedrockCallSum) -> Result<BedrockCall> {
    match bcs {
        BedrockCallSum::Claude3BCS { model_id, body } => Ok(BedrockCall::new(
            body.convert_to_blob()?,
            "application/json".to_string(),
            "*/*".to_string(),
            model_id,
        ))

    }
}

// Create a BedrockCallSum with sensible defaults for each model.
// This will fail if model_id is not known to q_to_bcs_with_defaults.
fn q_to_bcs_with_defaults(
    question: Option<String>,
    model_id: &str,
    image: Option<&Image>,
) -> Result<BedrockCallSum, anyhow::Error> {
    // call the function to load model settings:
    // TODO: do not hardcode the name and path of the config file
    let model_defaults = load_config(String::from("model_config.ron"))?;

    match model_id {
        
        "anthropic.claude-3-sonnet-20240229-v1:0" => {
            let claude_image: Option<ClaudeImageSource> = if image.is_some() {
                Some(ClaudeImageSource {
                    image_type: "base64".to_string(),
                    data: image.as_ref().unwrap().base64.clone(),
                    media_type: format!("image/{}", image.as_ref().unwrap().extension),
                })
            } else {
                None
            };
            let d = model_defaults.claude_v3;
            let claudev3_body = ClaudeV3Body::new(
                d.anthropic_version,
                d.max_tokens,
                d.role,
                d.default_content_type,
                question,
                claude_image,
            );
            Ok(BedrockCallSum::Claude3BCS {
                model_id: String::from("anthropic.claude-3-sonnet-20240229-v1:0"),
                body: claudev3_body,
            })
        }
        "anthropic.claude-3-haiku-20240307-v1:0" => {
            let claude_image: Option<ClaudeImageSource> = if image.is_some() {
                Some(ClaudeImageSource {
                    image_type: "base64".to_string(),
                    data: image.as_ref().unwrap().base64.clone(),
                    media_type: format!("image/{}", image.as_ref().unwrap().extension),
                })
            } else {
                None
            };
            let d = model_defaults.claude_v3;
            let claudev3_body = ClaudeV3Body::new(
                d.anthropic_version,
                d.max_tokens,
                d.role,
                d.default_content_type,
                question,
                claude_image,
            );
            Ok(BedrockCallSum::Claude3BCS {
                model_id: String::from("anthropic.claude-3-haiku-20240307-v1:0"),
                body: claudev3_body,
            })
        }

        
        &_ => todo!(),
    }
}

// Given a question and model_id, create a BedrockCall to this model.
// This will fail if model_id is not known to q_to_bcs_with_defaults.
fn mk_bedrock_call(
    question: &String,
    image: Option<&Image>,
    model_id: &str,
) -> Result<BedrockCall> {
    let bcs = q_to_bcs_with_defaults(Some(question.to_string()), model_id, image)?;
    bcs_to_bedrock_call(bcs)
}

// Given a question and model_id, create and execute a call to bedrock.
// This will fail if model_id is not known to q_to_bcs_with_defaults
pub async fn ask_bedrock(
    question: &String,
    image: Option<&Image>,
    model_id: &str,
    run_type: RunType,
    client: &aws_sdk_bedrockruntime::Client,
    bedrock_client: &aws_sdk_bedrock::Client,
) -> Result<String, anyhow::Error> {
    match run_type {
        RunType::Standard => {
            let bcall = mk_bedrock_call(question, image, model_id)?;
            // check if model supports streaming:
            if check_for_streaming(model_id.to_string(), bedrock_client).await? {
                let response = call_bedrock_stream(client, bcall).await?;
                Ok(response)
            } else {
                // if it does not just call it
                let response = call_bedrock(client, bcall, run_type).await?;
                Ok(response)
            }
        }
        RunType::Captioning => {
            if image.is_some() {
                // TODO: Programmaticall check for multimodality of FMs
                if model_id != "anthropic.claude-3-sonnet-20240229-v1:0"
                    && model_id != "anthropic.claude-3-haiku-20240307-v1:0"
                {
                    eprintln!("🛑SORRY! The model you selected is not able to caption images. Please select either `claude-v3-sonnet` or `claude-v3-haiku`.");
                    std::process::exit(1);
                }
                let bcall = mk_bedrock_call(question, image, model_id)?;
                // because this is captioniong, we dont need streaming
                let caption = call_bedrock(client, bcall, run_type).await?;
                Ok(caption)
            } else {
                Err(anyhow!(
                    "No images provided. Captioning aborted."
                ))
            }
        }
    }
    //Ok(())
}

//========================================

fn process_response(
    model_id: &str,
    payload_bytes: &[u8],
    streaming: bool,
) -> Result<String, serde_json::Error> {
    if !streaming {
        match model_id {
            "anthropic.claude-3-sonnet-20240229-v1:0"
            | "anthropic.claude-3-haiku-20240307-v1:0" => {
                serde_json::from_slice::<ClaudeV3Response>(payload_bytes)
                    .map(|res| res.content[0].text.clone())
            }
            &_ => Err(serde_json::Error::custom("Unknown model ID")),
        }
    } else {
        match model_id {
            "anthropic.claude-3-sonnet-20240229-v1:0"
            | "anthropic.claude-3-haiku-20240307-v1:0" => {
                // NOTE: ClaudeV3 is complicated and the streamed response is not always the same
                // this means we need to check for specific fields in the response and then return only
                // if we have the type of response set to "text_delta"
                // FIX: I feel like this could be way better
                // FIX: Make it so you check for other message types and to something about it.
                let mut deserializer = serde_json::Deserializer::from_slice(payload_bytes);
                let value = Value::deserialize(&mut deserializer)?;
                if let Value::Object(obj) = value {
                    if let Some(Value::Object(delta)) = obj.get("delta") {
                        if let Some(Value::String(delta_type)) = delta.get("type") {
                            if delta_type == "text_delta" {
                                let text = delta
                                    .get("text")
                                    .and_then(|v| v.as_str().map(ToString::to_string))
                                    .ok_or_else(|| Error::custom("text"))?;
                                return Ok(text);
                            }
                        }
                    }
                }
                Ok(String::from(""))
            }
            &_ => Err(serde_json::Error::custom("Unknown model ID")),
        }
    }
}

// this function is only called if we do not want the streaming result back.
// so far this is here only for models that do not support streaming (ie Jurrasic2Ultra)
async fn call_bedrock(
    bc: &aws_sdk_bedrockruntime::Client,
    c: BedrockCall,
    run_type: RunType,
) -> Result<String, anyhow::Error> {
    let response = bc
        .invoke_model()
        .body(c.body)
        .content_type(c.content_type)
        .accept(c.accept)
        .model_id(&c.model_id)
        .send()
        .await?;

    let response_text = process_response(c.model_id.as_str(), response.body.as_ref(), false);
    match response_text {
        Ok(text) => match run_type {
            RunType::Captioning => Ok(text),
            RunType::Standard => {
                println!("{}", text);
                Ok(text)
            }
        },
        Err(e) => Err(anyhow!("Error processing response: {}", e)),
    }
}

async fn call_bedrock_stream(bc: &aws_sdk_bedrockruntime::Client, c: BedrockCall) -> Result<String, anyhow::Error> {
    let mut resp = bc
        .invoke_model_with_response_stream()
        .body(c.body)
        .content_type(c.content_type)
        .accept(c.accept)
        .model_id(&c.model_id)
        .send()
        .await?;

    let mut output = String::new();

    while let Some(event) = resp.body.recv().await? {
        match event {
            ResponseStream::Chunk(payload_part) => {
                if let Some(payload_bytes) = payload_part.bytes {
                    let response_text =
                        process_response(c.model_id.as_str(), payload_bytes.as_ref(), true);
                    match response_text {
                        Ok(text) => {
                            output.push_str(&text);
                            print!("{}", &text);
                            io::stdout().flush()?;
                        }
                        Err(e) => eprintln!("Error processing response: {}", e),
                    }
                }
            }
            otherwise => panic!("received unexpected event type: {:?}", otherwise),
        }
    }
    println!();
    Ok(output)
}