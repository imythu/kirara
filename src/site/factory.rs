use reqwest::Client;

use super::{
    SiteAdapter, SiteAuth, SiteRecord, SiteType, mteam, nexusphp, parse_site_request_headers,
    site_request_header_map, unit3d,
};

pub fn create_adapter(record: &SiteRecord, client: Client) -> Result<Box<dyn SiteAdapter>, String> {
    create_adapter_with_cached_user_id(record, client, None)
}

pub fn create_adapter_with_cached_user_id(
    record: &SiteRecord,
    client: Client,
    cached_user_id: Option<&str>,
) -> Result<Box<dyn SiteAdapter>, String> {
    let site_type = SiteType::from_str(&record.site_type)
        .ok_or_else(|| format!("不支持的站点类型: {}", record.site_type))?;
    let auth = serde_json::from_str::<SiteAuth>(&record.auth_config)
        .map_err(|e| format!("认证配置解析失败: {}", e))?;
    let request_headers = parse_site_request_headers(&record.request_headers)?;
    let request_headers = site_request_header_map(&request_headers)?;

    Ok(match site_type {
        SiteType::Gazelle => Box::new(super::gazelle::GazelleAdapter::new(
            record.base_url.clone(), auth, request_headers, client,
        )),
        SiteType::NexusPhp => Box::new(
            nexusphp::NexusPhpAdapter::new(record.base_url.clone(), auth, request_headers, client)
                .with_cached_user_id(cached_user_id),
        ),
        SiteType::MTeam => Box::new(mteam::MTeamAdapter::new(
            record.base_url.clone(),
            auth,
            request_headers,
            client,
        )),
        SiteType::Unit3D => Box::new(unit3d::Unit3DAdapter::new(
            record.base_url.clone(),
            auth,
            request_headers,
            client,
        )),
    })
}
