use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalConfig {
    pub log_level: Option<String>,
    #[serde(default)]
    pub proxy: Option<String>,
    #[serde(default = "default_true")]
    pub use_proxy_for_lightpanda: bool,
    #[serde(default)]
    pub lightpanda: LightpandaConfig,
    #[serde(default)]
    pub browserless: BrowserlessConfig,
    #[serde(default = "default_tag_rule_scan_interval_mins")]
    pub tag_rule_scan_interval_mins: u64,
    #[serde(default)]
    pub vision_llm: VisionLlmConfig,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            log_level: Some("info".to_string()),
            proxy: None,
            use_proxy_for_lightpanda: true,
            lightpanda: LightpandaConfig::default(),
            browserless: BrowserlessConfig::default(),
            tag_rule_scan_interval_mins: default_tag_rule_scan_interval_mins(),
            vision_llm: VisionLlmConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LightpandaConfig {
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default = "default_lightpanda_region")]
    pub region: String,
    #[serde(default = "default_lightpanda_browser")]
    pub browser: String,
    #[serde(default = "default_lightpanda_proxy")]
    pub proxy: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
}

impl Default for LightpandaConfig {
    fn default() -> Self {
        Self {
            endpoint: None,
            token: None,
            region: default_lightpanda_region(),
            browser: default_lightpanda_browser(),
            proxy: default_lightpanda_proxy(),
            country: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BrowserlessConfig {
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
}

const fn default_true() -> bool {
    true
}

fn default_lightpanda_region() -> String {
    "euwest".to_string()
}

fn default_lightpanda_browser() -> String {
    "lightpanda".to_string()
}

fn default_lightpanda_proxy() -> Option<String> {
    Some("fast_dc".to_string())
}

const fn default_tag_rule_scan_interval_mins() -> u64 {
    7
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VisionLlmConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub api_standard: String,
    pub api_key_configured: bool,
    pub clear_api_key: bool,
}
impl Default for VisionLlmConfig {
    fn default() -> Self {
        Self {
            base_url: "https://openrouter.ai/api/v1".into(),
            model: String::new(),
            api_key: None,
            api_standard: "openai_responses".into(),
            api_key_configured: false,
            clear_api_key: false,
        }
    }
}
impl VisionLlmConfig {
    pub fn validate(&self) -> Result<(), String> {
        let url = reqwest::Url::parse(&self.base_url).map_err(|_| "视觉 LLM baseUrl 无效")?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("视觉 LLM baseUrl 必须是无凭据、查询参数或片段的 HTTP(S) 地址".into());
        }
        if !matches!(
            self.api_standard.as_str(),
            "claude" | "openai_compatible" | "openai_responses"
        ) {
            return Err("未知视觉 LLM 接口标准".into());
        }
        Ok(())
    }
    pub fn ready(&self) -> bool {
        !self.model.trim().is_empty()
            && self
                .api_key
                .as_deref()
                .is_some_and(|key| !key.trim().is_empty())
    }
}
