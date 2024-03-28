
pub mod claudev3;

use claudev3::ClaudeV3Config;

use anyhow::{anyhow, Result};
use aws_sdk_bedrock::{self, types::FoundationModelDetails};
use serde::{Deserialize, Serialize};

use std::fs;

#[derive(Debug, Deserialize, Serialize)]
pub struct ModelConfigs {
    pub claude_v3: ClaudeV3Config
}

pub fn load_config(f: String) -> Result<ModelConfigs> {
    let file = fs::File::open(f)?;
    let config: ModelConfigs = ron::de::from_reader(file)?;
    Ok(config)
}

pub async fn check_for_streaming(
    m: String,
    c: &aws_sdk_bedrock::Client,
) -> Result<bool, anyhow::Error> {
    let call = c.get_foundation_model().model_identifier(m);
    let res = call.send().await;
    let model_details: FoundationModelDetails = res?
        .model_details()
        .ok_or_else(|| anyhow!("Unable to get model details"))?
        .clone();

    match model_details.response_streaming_supported {
        Some(o) => Ok(o),
        None => Ok(false),
    }
}