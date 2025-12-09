use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default = "default_bind_address")]
    pub bind_address: String,
    #[serde(default = "default_bind_port")]
    pub bind_port: u16,
    pub samples_dir: String,
    #[serde(default = "default_laslist_file")]
    pub laslist_file: String,
    pub html_row_steps: usize,
    pub pixels_per_step: usize,
    pub image_width: usize,
    pub scale_spacing: usize,
    #[serde(default = "default_max_scales")]
    pub max_scales: usize,
    #[serde(default = "default_tick_size_major")]
    pub tick_size_major: usize,
    #[serde(default = "default_tick_size_minor")]
    pub tick_size_minor: usize,
    pub default_colors: Vec<String>,
    pub separate_depth_column: bool,
}

fn default_bind_address() -> String {
    "127.0.0.1".to_string()
}

fn default_bind_port() -> u16 {
    8080
}

fn default_laslist_file() -> String {
    "lasfiles.txt".to_string()
}

fn default_max_scales() -> usize {
    6
}

fn default_tick_size_major() -> usize {
    8
}

fn default_tick_size_minor() -> usize {
    4
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let config_str = std::fs::read_to_string("lasplot.toml")?;
        let config: Config = toml::from_str(&config_str)?;
        Ok(config)
    }

    pub fn get_samples_path(&self) -> PathBuf {
        PathBuf::from(&self.samples_dir)
    }
}

