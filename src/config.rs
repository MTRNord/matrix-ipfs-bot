use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub ipfs_gateway: String,
    pub ipfs_api: String,
}

impl Config {
    pub fn load() -> Self {
        let f = OpenOptions::new().read(true).open("./config.yml").unwrap();
        let json: Self = serde_yaml::from_reader(f).expect("config should be proper YAML");
        json
    }
}
