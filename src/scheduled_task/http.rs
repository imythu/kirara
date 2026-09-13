use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

const MAX_BODY: usize = 262144;
#[derive(Clone, Serialize, Deserialize)]
pub struct Pair {
    pub name: String,
    pub value: String,
}
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Auth {
    #[default]
    None,
    Bearer {
        token: String,
    },
    Basic {
        username: String,
        password: String,
    },
    ApiKey {
        name: String,
        value: String,
        location: String,
    },
    Cookie {
        value: String,
    },
}
#[derive(Clone, Serialize, Deserialize)]
pub struct FilePayload {
    pub name: String,
    pub content_type: String,
    pub data_base64: String,
}
impl FilePayload {
    fn bytes(&self) -> Result<Vec<u8>, String> {
        if self.data_base64.len() > MAX_BODY.div_ceil(3) * 4 {
            return Err("文件最大 256 KiB".into());
        }
        let bytes = STANDARD
            .decode(&self.data_base64)
            .map_err(|_| "文件编码无效，请重新选择文件".to_string())?;
        if bytes.len() > MAX_BODY {
            return Err("文件最大 256 KiB".into());
        }
        Ok(bytes)
    }
    fn validate(&self) -> Result<usize, String> {
        if self.name.is_empty() || self.name.len() > 255 || self.name.contains(['\r', '\n', '\0']) {
            return Err("文件名无效".into());
        }
        reqwest::multipart::Part::bytes(Vec::new())
            .mime_str(&self.content_type)
            .map_err(|_| "文件 MIME 类型无效".to_string())?;
        Ok(self.bytes()?.len())
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct FormField {
    pub name: String,
    #[serde(default)]
    pub value: String,
    pub file: Option<FilePayload>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct HttpConfig {
    #[serde(default)]
    pub send_via_browser: bool,
    #[serde(default)]
    pub use_global_proxy: bool,
    #[serde(default)]
    pub browser_use_global_proxy: bool,
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub query: Vec<Pair>,
    #[serde(default)]
    pub headers: Vec<Pair>,
    // Read old records without migrating credentials or changing their behavior.
    #[serde(default)]
    pub bearer_token: String,
    #[serde(default)]
    pub auth: Option<Auth>,
    #[serde(default)]
    pub body: String,
    pub body_type: String,
    #[serde(default)]
    pub form_fields: Vec<FormField>,
    #[serde(default)]
    pub binary: Option<FilePayload>,
    #[serde(default)]
    pub content_type: String,
    pub timeout_seconds: u64,
    pub follow_redirects: bool,
    pub expected_status: Option<u16>,
}
impl HttpConfig {
    pub fn effective_auth(&self) -> Auth {
        self.auth.clone().unwrap_or_else(|| {
            if self.bearer_token.is_empty() {
                Auth::None
            } else {
                Auth::Bearer {
                    token: self.bearer_token.clone(),
                }
            }
        })
    }
    fn auth_header(&self) -> Result<Option<Pair>, String> {
        Ok(match self.effective_auth() {
            Auth::None => None,
            Auth::Bearer { token } => {
                if token.trim().is_empty() {
                    return Err("请填写 Bearer Token".into());
                }
                Some(Pair {
                    name: "Authorization".into(),
                    value: format!("Bearer {token}"),
                })
            }
            Auth::Basic { username, password } => {
                if username.contains(':') {
                    return Err("Basic Auth 用户名不能包含冒号".into());
                }
                Some(Pair {
                    name: "Authorization".into(),
                    value: format!(
                        "Basic {}",
                        STANDARD.encode(format!("{username}:{password}"))
                    ),
                })
            }
            Auth::ApiKey {
                name,
                value,
                location,
            } => {
                if name.trim().is_empty() || value.is_empty() {
                    return Err("请填写 API Key 名称和值".into());
                }
                match location.as_str() {
                    "header" => Some(Pair { name, value }),
                    "query" => None,
                    _ => return Err("API Key 位置应为请求头或查询参数".into()),
                }
            }
            Auth::Cookie { value } => {
                if value.trim().is_empty() {
                    return Err("请填写 Cookie".into());
                }
                Some(Pair {
                    name: "Cookie".into(),
                    value,
                })
            }
        })
    }
    pub fn validate(&self) -> Result<(), String> {
        let url = reqwest::Url::parse(&self.url)
            .map_err(|_| "请输入完整的 HTTP / HTTPS 地址".to_string())?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err("仅支持 HTTP / HTTPS，请通过认证面板填写凭据".into());
        }
        if !["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"]
            .contains(&self.method.as_str())
        {
            return Err("不支持的 HTTP 方法".into());
        }
        if !(1..=300).contains(&self.timeout_seconds) {
            return Err("超时应为 1–300 秒".into());
        }
        if self
            .expected_status
            .is_some_and(|s| !(100..=599).contains(&s))
        {
            return Err("预期状态码应为 100–599".into());
        }
        if self.headers.len() > 100
            || self.query.len() > 100
            || self.form_fields.len() > 100
            || self.body.len() > MAX_BODY
            || self.url.len() > 8192
        {
            return Err("最多 100 条参数/请求头/表单字段，请求体最大 256 KiB".into());
        }
        if ![
            "none",
            "json",
            "text",
            "xml",
            "html",
            "urlencoded",
            "multipart",
            "binary",
            "raw",
        ]
        .contains(&self.body_type.as_str())
        {
            return Err("不支持的请求体类型".into());
        }
        if self.body_type == "json"
            && serde_json::from_str::<serde_json::Value>(&self.body).is_err()
        {
            return Err("请求体不是有效的 JSON".into());
        }
        if ["GET", "HEAD"].contains(&self.method.as_str()) && self.body_type != "none" {
            return Err("GET / HEAD 请使用查询参数，不支持请求体".into());
        }
        let auth_header = self.auth_header()?;
        for p in self.headers.iter().chain(auth_header.iter()) {
            validate_header(p)?;
            if self.body_type == "multipart" && p.name.eq_ignore_ascii_case("content-type") {
                return Err("multipart 的 Content-Type 和 boundary 会自动生成，请移除手动设置的 Content-Type".into());
            }
        }
        if let Some(p) = auth_header {
            if self
                .headers
                .iter()
                .any(|h| h.name.eq_ignore_ascii_case(&p.name))
            {
                return Err(format!("认证已生成 {}，请移除重复的请求头", p.name));
            }
        }
        if let Auth::ApiKey { name, location, .. } = self.effective_auth() {
            if location == "query"
                && (self.query.iter().any(|p| p.name == name)
                    || url.query_pairs().any(|(k, _)| k == name))
            {
                return Err("API Key 与地址或查询参数中的同名参数重复，请只保留一个".into());
            }
        }
        if self.query.iter().any(|p| p.name.trim().is_empty()) {
            return Err("查询参数名称不能为空".into());
        }
        if ["urlencoded", "multipart"].contains(&self.body_type.as_str()) {
            let mut size = 0;
            for p in &self.form_fields {
                if p.name.trim().is_empty() || p.name.contains(['\r', '\n', '\0']) {
                    return Err("表单字段名称无效".into());
                }
                size += p.name.len();
                size += if let Some(file) = &p.file {
                    if self.body_type == "urlencoded" {
                        return Err("URL 编码表单不支持文件，请选择 multipart".into());
                    }
                    file.validate()?
                } else {
                    p.value.len()
                };
            }
            if size > MAX_BODY {
                return Err("表单内容和文件合计最大 256 KiB".into());
            }
        }
        if self.body_type == "binary" {
            self.binary.as_ref().ok_or("请选择二进制文件")?.validate()?;
        }
        if self.body_type == "raw" {
            if self.content_type.trim().is_empty() {
                return Err("请填写自定义 Content-Type".into());
            }
            validate_header(&Pair {
                name: "Content-Type".into(),
                value: self.content_type.clone(),
            })?;
        }
        Ok(())
    }
    pub fn summary(&self) -> String {
        let host = reqwest::Url::parse(&self.url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_default();
        format!("{} · {}", self.method, host)
    }
    pub fn build_request(&self, client: &reqwest::Client) -> Result<reqwest::Request, String> {
        self.validate()?;
        let mut request = client
            .request(
                reqwest::Method::from_bytes(self.method.as_bytes()).map_err(|_| "无效的方法")?,
                &self.url,
            )
            .query(
                &self
                    .query
                    .iter()
                    .map(|p| (&p.name, &p.value))
                    .collect::<Vec<_>>(),
            );
        for pair in &self.headers {
            request = request.header(&pair.name, &pair.value);
        }
        if let Some(pair) = self.auth_header()? {
            request = request.header(&pair.name, &pair.value);
        }
        if let Auth::ApiKey {
            name,
            value,
            location,
        } = self.effective_auth()
        {
            if location == "query" {
                request = request.query(&[(name, value)]);
            }
        }
        let mime = match self.body_type.as_str() {
            "none" | "multipart" => None,
            "json" => Some("application/json"),
            "text" => Some("text/plain; charset=utf-8"),
            "xml" => Some("application/xml"),
            "html" => Some("text/html; charset=utf-8"),
            "urlencoded" => Some("application/x-www-form-urlencoded"),
            "binary" => Some(self.binary.as_ref().unwrap().content_type.as_str()),
            "raw" => Some(self.content_type.as_str()),
            _ => unreachable!(),
        };
        if let Some(mime) = mime {
            if !self
                .headers
                .iter()
                .any(|p| p.name.eq_ignore_ascii_case("content-type"))
            {
                request = request.header("Content-Type", mime);
            }
        }
        request = match self.body_type.as_str() {
            "none" => request,
            "urlencoded" => request.form(
                &self
                    .form_fields
                    .iter()
                    .map(|p| (&p.name, &p.value))
                    .collect::<Vec<_>>(),
            ),
            "multipart" => {
                let mut form = reqwest::multipart::Form::new();
                for p in &self.form_fields {
                    form = if let Some(file) = &p.file {
                        form.part(
                            p.name.clone(),
                            reqwest::multipart::Part::bytes(file.bytes()?)
                                .file_name(file.name.clone())
                                .mime_str(&file.content_type)
                                .map_err(|_| "文件 MIME 类型无效")?,
                        )
                    } else {
                        form.text(p.name.clone(), p.value.clone())
                    };
                }
                request.multipart(form)
            }
            "binary" => request.body(self.binary.as_ref().unwrap().bytes()?),
            _ => request.body(self.body.clone()),
        };
        request
            .build()
            .map_err(|_| "无法构建请求，请检查配置".into())
    }
}
fn validate_header(p: &Pair) -> Result<(), String> {
    let name =
        reqwest::header::HeaderName::from_str(&p.name).map_err(|_| "请求头名称无效".to_string())?;
    reqwest::header::HeaderValue::from_str(&p.value)
        .map_err(|_| "请求头值无效（不可包含换行）".to_string())?;
    if ["host", "content-length", "transfer-encoding", "connection"].contains(&name.as_str()) {
        return Err("Host、Content-Length、Transfer-Encoding、Connection 由客户端管理".into());
    }
    Ok(())
}
#[derive(Serialize)]
pub struct RequestPreview {
    method: String,
    url: String,
    headers: Vec<Pair>,
    body: String,
}
pub fn request_preview(config: &HttpConfig) -> Result<RequestPreview, String> {
    let request = config.build_request(&reqwest::Client::new())?;
    let body = match config.body_type.as_str() {
        "multipart" => config
            .form_fields
            .iter()
            .map(|p| {
                if let Some(f) = &p.file {
                    format!(
                        "{}: [文件 {} · {} · {} 字节]",
                        p.name,
                        f.name,
                        f.content_type,
                        f.bytes().unwrap().len()
                    )
                } else {
                    format!("{}: {}", p.name, p.value)
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        "binary" => {
            let f = config.binary.as_ref().unwrap();
            format!(
                "[文件 {} · {} · {} 字节]",
                f.name,
                f.content_type,
                f.bytes()?.len()
            )
        }
        _ => request
            .body()
            .and_then(|b| b.as_bytes())
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default(),
    };
    Ok(RequestPreview {
        method: request.method().to_string(),
        url: request.url().to_string(),
        headers: request
            .headers()
            .iter()
            .map(|(k, v)| Pair {
                name: k.to_string(),
                value: v.to_str().unwrap_or("[非文本值]").into(),
            })
            .collect(),
        body,
    })
}
