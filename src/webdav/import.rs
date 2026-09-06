//! Cookie normalization shared by the receiver and its persistent worker.
use reqwest::{Url, header::HeaderValue};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read},
};

pub const MAX_BODY: usize = 8 * 1024 * 1024;
const MAX_COOKIES: usize = 10_000;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cookie {
    domain: String,
    name: String,
    value: String,
    #[serde(default = "root_path")]
    path: String,
    #[serde(default)]
    host_only: bool,
    #[serde(default)]
    expiration_date: Option<f64>,
    #[serde(default)]
    session: bool,
    #[serde(default)]
    secure: bool,
}
fn root_path() -> String {
    "/".into()
}

#[derive(Clone)]
pub struct Candidate {
    pub ptd_id: String,
    pub name: String,
    pub site_type: String,
    pub base_url: String,
    pub cookie: String,
    // Keep scopes for updating a pre-existing alternative entry point correctly.
    cookies: Vec<Cookie>,
}

pub struct ImportData {
    pub generated_at: Option<i64>,
    pub cookies: BTreeMap<String, Vec<Cookie>>,
}

fn read_entry(zip: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Result<Vec<u8>, String> {
    let entry = zip.by_name(name).map_err(|_| format!("缺少 {name}"))?;
    if entry.size() > MAX_BODY as u64 {
        return Err("展开后的数据超过 8 MiB".into());
    }
    let mut bytes = Vec::new();
    entry
        .take(MAX_BODY as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "读取备份数据失败")?;
    if bytes.len() > MAX_BODY {
        return Err("展开后的数据超过 8 MiB".into());
    }
    Ok(bytes)
}

pub fn parse(bytes: &[u8]) -> Result<ImportData, String> {
    if bytes.len() > MAX_BODY {
        return Err("数据超过 8 MiB".into());
    }
    let (raw, generated_at) = if bytes.starts_with(b"PK") {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| "备份格式无效")?;
        if zip.len() > 32 {
            return Err("备份中的文件过多".into());
        }
        let mut names = BTreeSet::new();
        for name in zip.file_names() {
            if !names.insert(name.to_string()) {
                return Err("备份包含重复文件名".into());
            }
        }
        let manifest: serde_json::Value =
            serde_json::from_slice(&read_entry(&mut zip, "manifest.json")?)
                .map_err(|_| "manifest.json 格式无效")?;
        if manifest["encryption"].as_bool() != Some(false) {
            return Err("请在 PTD 中清空备份加密密钥，重新导出或推送".into());
        }
        let file = &manifest["files"]["cookies"];
        if file["name"].as_str() != Some("cookies.json") {
            return Err("备份未包含 cookies.json，请在 PTD 中勾选 Cookie".into());
        }
        let raw = read_entry(&mut zip, "cookies.json")?;
        if file["hash"].as_str() != Some(format!("{:x}", md5::compute(&raw)).as_str()) {
            return Err("Cookie 数据校验失败".into());
        }
        let timestamp = manifest["time"].as_i64().ok_or("备份生成时间无效")?;
        if timestamp <= 0 || timestamp > chrono::Utc::now().timestamp_millis() + 300_000 {
            return Err("备份生成时间无效，请检查发送设备的时钟".into());
        }
        (raw, Some(timestamp))
    } else {
        (bytes.to_vec(), None)
    };
    let cookies: BTreeMap<String, Vec<Cookie>> =
        serde_json::from_slice(&raw).map_err(|_| "Cookie 数据结构无效")?;
    if cookies.len() > 500 || cookies.values().map(Vec::len).sum::<usize>() > MAX_COOKIES {
        return Err("Cookie 数据条目过多".into());
    }
    Ok(ImportData {
        generated_at,
        cookies,
    })
}

pub fn host(url: &str) -> Option<String> {
    Url::parse(url)
        .ok()?
        .host_str()
        .map(str::to_ascii_lowercase)
}
pub fn site_key(url: &str) -> Option<String> {
    let host = host(url)?;
    Some(
        crate::ptd_sites::site_id_for_host(&host)
            .unwrap_or(&host)
            .to_string(),
    )
}

pub fn cookie_header(cookies: &[Cookie], target: &str) -> Option<String> {
    let https = Url::parse(target).ok()?.scheme() == "https";
    let target = host(target)?;
    let now = chrono::Utc::now().timestamp() as f64;
    let mut pairs = BTreeMap::<String, (bool, String)>::new();
    for c in cookies {
        let domain = c.domain.trim_start_matches('.').to_ascii_lowercase();
        let matches = if c.host_only {
            target == domain
        } else {
            target == domain || target.ends_with(&format!(".{domain}"))
        };
        if !matches
            || c.path != "/"
            || (c.secure && !https)
            || (!c.session
                && c.expiration_date
                    .is_some_and(|t| !t.is_finite() || t <= now))
        {
            continue;
        }
        if c.name.is_empty()
            || !c
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
            || c.value
                .bytes()
                .any(|b| b < 0x21 || b > 0x7e || b"\";,\\".contains(&b))
        {
            continue;
        }
        match pairs.get(&c.name) {
            Some((specific, value)) if value != &c.value => {
                if *specific == c.host_only {
                    return None;
                }
                if *specific {
                    continue;
                }
            }
            _ => {}
        }
        pairs.insert(c.name.clone(), (c.host_only, c.value.clone()));
    }
    // A logged-out browser may retain analytics, csrf and SSL preferences. Those
    // must not replace a working credential after the login cookie expires.
    if !pairs.iter().any(|(key, (_, value))| {
        let key = key.to_ascii_lowercase();
        !value.is_empty()
            && (key.contains("session")
                || key.ends_with("_sid")
                || matches!(
                    key.as_str(),
                    "c_secure_pass"
                        | "nexusphp_u2"
                        | "pass"
                        | "passhash"
                        | "auth"
                        | "auth_token"
                        | "authkey"
                        | "token"
                        | "rememberme"
                        | "sid"
                ))
    }) {
        return None;
    }
    let value = pairs
        .into_iter()
        .map(|(k, (_, v))| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("; ");
    (value.len() <= 32 * 1024 && HeaderValue::from_str(&value).is_ok()).then_some(value)
}

pub fn normalize(data: &ImportData) -> (Vec<Candidate>, Vec<String>) {
    let mut result = Vec::new();
    let mut skipped = Vec::new();
    for (domain, cookies) in &data.cookies {
        let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
        let url = format!("https://{domain}");
        if host(&url).as_deref() != Some(domain.as_str())
            || domain.contains(['/', ':', '@', '?', '#'])
        {
            skipped.push("跳过无效站点域名".into());
            continue;
        }
        let Some(id) = crate::ptd_sites::site_id_for_host(&domain) else {
            skipped.push(format!("{domain}：未识别站点"));
            continue;
        };
        let Some(preset) = crate::ptd_site_catalog::SITE_PRESETS
            .iter()
            .find(|p| p.ptd_id == id && p.site_type != "mteam")
        else {
            skipped.push(format!("{domain}：暂无 Cookie 适配预设"));
            continue;
        };
        let Some(cookie) = cookie_header(cookies, &url) else {
            skipped.push(format!("{domain}：没有可用的 Cookie"));
            continue;
        };
        // Keep alternative domains available for matching an existing site; deduplicate
        // site writes in the transaction instead of merging credentials across hosts.
        result.push(Candidate {
            ptd_id: id.into(),
            name: preset.name.into(),
            site_type: preset.site_type.into(),
            base_url: url,
            cookie,
            cookies: cookies.clone(),
        });
    }
    (result, skipped)
}

pub fn header_for(candidate: &Candidate, url: &str) -> Option<String> {
    cookie_header(&candidate.cookies, url)
}
