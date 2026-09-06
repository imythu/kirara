use super::*;
use crate::site::{SiteAuth, default_site_request_headers};
use serde_json::{Value, json};
use std::io::{Cursor, Write};

const PASSWORD: &str = "synthetic-test-password";
async fn database() -> (tempfile::TempDir, Database) {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path()).await.unwrap();
    configure(&db, true, "update").await;
    (dir, db)
}
async fn configure(db: &Database, enabled: bool, policy: &str) {
    let cfg = Config {
        enabled,
        existing_policy: policy.into(),
        password_hash: store::digest(PASSWORD.as_bytes()),
        ..Config::default()
    };
    db.dav(move |conn| {
        conn.execute(
            "UPDATE webdav_sync_settings SET config=? WHERE id=1",
            [serde_json::to_string(&cfg).unwrap()],
        )
        .map_err(sql)?;
        Ok(())
    })
    .await
    .unwrap();
}
fn cookies(value: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({"hdhome.org":[{"domain":"hdhome.org","hostOnly":true,"name":"c_secure_pass","value":value,"path":"/","secure":true}]})).unwrap()
}
fn archive(body: &[u8], time: i64) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("cookies.json", options).unwrap();
    zip.write_all(body).unwrap();
    zip.start_file("manifest.json", options).unwrap();
    zip.write_all(serde_json::to_string(&json!({"encryption":false,"time":time,"files":{"cookies":{"name":"cookies.json","hash":format!("{:x}",md5::compute(body))}}})).unwrap().as_bytes()).unwrap();
    zip.finish().unwrap().into_inner()
}
async fn receive(db: &Database, name: &str, bytes: Vec<u8>) -> bool {
    let name = name.to_string();
    let time = import::parse(&bytes).unwrap().generated_at;
    db.dav(move |conn| store::receive(conn, name, bytes, time, store::digest(PASSWORD.as_bytes())))
        .await
        .unwrap()
}
async fn process(db: &Database) {
    assert!(db.dav(store::process_next).await.unwrap());
}
async fn reports(db: &Database) -> Vec<Value> {
    db.dav(|conn| {
        let mut stmt = conn
            .prepare("SELECT result FROM webdav_sync_jobs ORDER BY id")
            .map_err(sql)?;
        let reports = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(sql)?
            .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
            .collect();
        Ok(reports)
    })
    .await
    .unwrap()
}

#[test]
fn parsing_checks_archive_integrity_and_cookie_scope() {
    let data = archive(&cookies("safe"), chrono::Utc::now().timestamp_millis());
    let parsed = import::parse(&data).unwrap();
    let (sites, skipped) = import::normalize(&parsed);
    assert!(skipped.is_empty());
    assert_eq!(sites.len(), 1);
    assert_eq!(sites[0].cookie, "c_secure_pass=safe");
    assert!(import::header_for(&sites[0], "https://attacker.test").is_none());
    assert!(import::header_for(&sites[0], "http://hdhome.org").is_none());
    assert!(import::parse(b"not JSON").is_err());
    let mut damaged = data;
    damaged.truncate(damaged.len() / 2);
    assert!(import::parse(&damaged).is_err());
    let too_large = vec![b' '; import::MAX_BODY + 1];
    assert!(import::parse(&archive(&too_large, 1)).is_err());
    let scopes = json!({"open.cd":[{"domain":".open.cd","name":"_ga","value":"analytics"},{"domain":"www.open.cd","hostOnly":true,"name":"c_secure_pass","value":"safe"}],"www.open.cd":[{"domain":"www.open.cd","hostOnly":true,"name":"c_secure_pass","value":"safe"}]});
    let parsed = import::parse(&serde_json::to_vec(&scopes).unwrap()).unwrap();
    let (sites, _) = import::normalize(&parsed);
    assert_eq!(sites.len(), 1);
    assert_eq!(sites[0].base_url, "https://www.open.cd");
    assert!(import::header_for(&sites[0], "https://open.cd").is_none());
    let logged_out = json!({"HDHOME.ORG.":[
        {"domain":"hdhome.org","name":"c_secure_ssl","value":"yes"},
        {"domain":"hdhome.org","name":"c_secure_pass","value":"expired","expirationDate":1},
        {"domain":"hdhome.org","name":"nexus_csrf_token","value":"not-auth"}
    ]});
    let parsed = import::parse(&serde_json::to_vec(&logged_out).unwrap()).unwrap();
    assert!(import::normalize(&parsed).0.is_empty());
}

#[tokio::test]
async fn updates_preserve_site_identity_and_reject_older_backups() {
    let (_dir, db) = database().await;
    let auth = serde_json::to_string(&SiteAuth::CookiePasskey {
        cookie: "c_secure_pass=old".into(),
        passkey: "keep-passkey".into(),
    })
    .unwrap();
    let id = db
        .create_site(
            "自定义名称",
            "nexusphp",
            "https://hdhome.org",
            &auth,
            "[]",
            false,
        )
        .await
        .unwrap();
    let time = chrono::Utc::now().timestamp_millis();
    receive(&db, "new.zip", archive(&cookies("new"), time)).await;
    process(&db).await;
    let site = db.get_site(id).await.unwrap().unwrap();
    assert_eq!(site.name, "自定义名称");
    assert!(!site.use_proxy);
    assert_eq!(site.request_headers, "[]");
    let auth: Value = serde_json::from_str(&site.auth_config).unwrap();
    assert_eq!(auth["passkey"], "keep-passkey");
    assert_eq!(auth["cookie"], "c_secure_pass=new");
    receive(&db, "older.zip", archive(&cookies("stale"), time - 1)).await;
    process(&db).await;
    assert_eq!(
        db.get_site(id).await.unwrap().unwrap().auth_config,
        site.auth_config
    );
    receive(&db, "same.json", cookies("new")).await;
    process(&db).await;
    assert_eq!(
        db.get_site(id).await.unwrap().unwrap().updated_at,
        site.updated_at
    );
    let reports = reports(&db).await;
    assert_eq!(reports[0]["updated"], 1);
    assert_eq!(reports[1]["skipped"], 1);
    assert_eq!(reports[2]["unchanged"], 1);
}

#[tokio::test]
async fn duplicate_upload_and_restart_preserve_pending_work() {
    let (dir, db) = database().await;
    assert!(receive(&db, "cookies.json", cookies("one")).await);
    assert!(!receive(&db, "cookies.json", cookies("one")).await);
    // Cover overwriting the same path before the earlier job has run.
    assert!(!receive(&db, "cookies.json", cookies("two")).await);
    drop(db);
    let db = Database::open(dir.path()).await.unwrap();
    process(&db).await;
    process(&db).await;
    assert!(!db.dav(store::process_next).await.unwrap());
    let sites = db.list_sites().await.unwrap();
    assert_eq!(sites.len(), 1);
    assert!(sites[0].auth_config.contains("two"));
    let reports = reports(&db).await;
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0]["created"], 1);
    assert_eq!(reports[1]["updated"], 1);
}

#[tokio::test]
async fn skip_policy_and_ambiguous_matches_do_not_overwrite() {
    let (_dir, db) = database().await;
    let headers = serde_json::to_string(&default_site_request_headers()).unwrap();
    let auth = r#"{"auth_type":"cookie","cookie":"original=1"}"#;
    db.create_site(
        "first",
        "nexusphp",
        "https://hdhome.org",
        auth,
        &headers,
        false,
    )
    .await
    .unwrap();
    configure(&db, true, "skip").await;
    receive(&db, "a.json", cookies("ignored")).await;
    process(&db).await;
    assert_eq!(db.list_sites().await.unwrap()[0].auth_config, auth);
    configure(&db, true, "update").await;
    db.create_site(
        "second",
        "nexusphp",
        "https://www.hdhome.org",
        auth,
        &headers,
        false,
    )
    .await
    .unwrap();
    receive(&db, "b.json", cookies("ambiguous")).await;
    process(&db).await;
    assert!(
        db.list_sites()
            .await
            .unwrap()
            .iter()
            .all(|s| s.auth_config == auth)
    );
    assert_eq!(reports(&db).await[1]["skipped"], 1);
}

#[tokio::test]
async fn failure_rolls_back_sites_and_job_progress_together() {
    let (_dir, db) = database().await;
    receive(&db, "cookies.json", cookies("one")).await;
    db.dav(|conn|{conn.execute_batch("CREATE TRIGGER fail_dav BEFORE UPDATE OF status ON webdav_sync_jobs WHEN NEW.status='done' BEGIN SELECT RAISE(ABORT,'test failure'); END;").map_err(sql)}).await.unwrap();
    assert!(db.dav(store::process_next).await.is_err());
    assert!(db.list_sites().await.unwrap().is_empty());
    db.dav(|conn| conn.execute_batch("DROP TRIGGER fail_dav").map_err(sql))
        .await
        .unwrap();
    process(&db).await;
    assert_eq!(db.list_sites().await.unwrap().len(), 1);
}

#[tokio::test]
async fn queue_bound_rejects_without_losing_accepted_jobs() {
    let (_dir, db) = database().await;
    db.dav(|conn| {
        let tx=conn.transaction().map_err(sql)?;
        for i in 0..100 {tx.execute("INSERT INTO webdav_sync_jobs(name,body,digest,received_at) VALUES(?,X'7B7D','fixture',0)",[format!("{i}.json")]).map_err(sql)?;}
        tx.commit().map_err(sql)
    }).await.unwrap();
    let error = db
        .dav(|conn| {
            store::receive(
                conn,
                "next.json".into(),
                cookies("safe"),
                None,
                store::digest(PASSWORD.as_bytes()),
            )
        })
        .await
        .unwrap_err();
    assert!(error.contains("100"));
    let count = db
        .dav(|conn| {
            conn.query_row("SELECT COUNT(*) FROM webdav_sync_jobs", [], |r| {
                r.get::<_, i64>(0)
            })
            .map_err(sql)
        })
        .await
        .unwrap();
    assert_eq!(count, 100);
}

async fn serve(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (url, handle)
}

#[tokio::test]
async fn webdav_auth_upload_listing_download_delete_and_management_redaction() {
    let (_dir, db) = database().await;
    let stop = CancellationToken::new();
    let (base, server) = serve(protocol::router(db.clone(), stop.clone())).await;
    let client = reqwest::Client::new();
    let url = format!("{base}/dav/ptd/cookies.json");
    assert_eq!(
        client
            .put(&url)
            .body(cookies("safe"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        client
            .request(
                reqwest::Method::from_bytes(b"PROPFIND").unwrap(),
                format!("{base}/dav/ptd/")
            )
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        client
            .put(&url)
            .basic_auth("ptd", Some(PASSWORD))
            .body(cookies("safe"))
            .send()
            .await
            .unwrap()
            .status(),
        201
    );
    assert_eq!(
        client
            .put(&url)
            .basic_auth("ptd", Some(PASSWORD))
            .body(cookies("safe"))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    let prop = client
        .request(
            reqwest::Method::from_bytes(b"PROPFIND").unwrap(),
            format!("{base}/dav/ptd/"),
        )
        .basic_auth("ptd", Some(PASSWORD))
        .header("Depth", "1")
        .send()
        .await
        .unwrap();
    assert_eq!(prop.status(), 207);
    let listing = prop.text().await.unwrap();
    assert!(listing.contains("cookies.json"));
    assert!(!listing.contains("c_secure_pass"));
    let downloaded = client
        .get(&url)
        .basic_auth("ptd", Some(PASSWORD))
        .send()
        .await
        .unwrap();
    assert_eq!(downloaded.bytes().await.unwrap().as_ref(), cookies("safe"));
    let head = client
        .head(&url)
        .basic_auth("ptd", Some(PASSWORD))
        .send()
        .await
        .unwrap();
    assert_eq!(head.status(), 200);
    assert_eq!(
        head.headers()["content-length"]
            .to_str()
            .unwrap()
            .parse::<usize>()
            .unwrap(),
        cookies("safe").len()
    );
    assert!(head.bytes().await.unwrap().is_empty());
    assert_eq!(
        client
            .put(format!("{base}/dav/ptd/bad.json"))
            .basic_auth("ptd", Some(PASSWORD))
            .body("malformed")
            .send()
            .await
            .unwrap()
            .status(),
        422
    );
    assert_eq!(
        client
            .get(format!("{base}/api/sites"))
            .basic_auth("ptd", Some(PASSWORD))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        client
            .delete(&url)
            .basic_auth("ptd", Some(PASSWORD))
            .send()
            .await
            .unwrap()
            .status(),
        204
    );
    // Deleting a DAV resource cannot cancel its durable sync job.
    process(&db).await;
    assert_eq!(db.list_sites().await.unwrap().len(), 1);
    let (admin, admin_handle) = serve(management_router(db.clone())).await;
    let settings = client
        .get(format!("{admin}/api/sites/webdav-sync"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!settings.contains(PASSWORD));
    assert!(!settings.contains(&store::digest(PASSWORD.as_bytes())));
    let runs = client
        .get(format!("{admin}/api/sites/webdav-sync/runs"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!runs.contains("c_secure_pass"));
    assert!(!runs.contains("safe"));
    configure(&db, false, "update").await;
    assert_eq!(
        client
            .get(&url)
            .basic_auth("ptd", Some(PASSWORD))
            .send()
            .await
            .unwrap()
            .status(),
        503
    );
    stop.cancel();
    server.abort();
    admin_handle.abort();
}

#[tokio::test]
async fn configuration_generates_password_once_and_rotation_rejects_old_password() {
    let (_dir, db) = database().await;
    let (admin, admin_handle) = serve(management_router(db.clone())).await;
    let stop = CancellationToken::new();
    let (dav, dav_handle) = serve(protocol::router(db.clone(), stop.clone())).await;
    let client = reqwest::Client::new();
    let body = json!({"enabled":true,"username":"ptd","existing_policy":"update","auto_create":true,"rotate_password":true});
    let response = client
        .put(format!("{admin}/api/sites/webdav-sync"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let value: Value = response.json().await.unwrap();
    let password = value["new_password"].as_str().unwrap();
    assert_eq!(password.len(), 64);
    assert_eq!(
        client
            .get(format!("{dav}/dav/ptd/"))
            .basic_auth("ptd", Some(PASSWORD))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        client
            .get(format!("{dav}/dav/ptd/"))
            .basic_auth("ptd", Some(password))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let value: Value = client
        .get(format!("{admin}/api/sites/webdav-sync"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(value.get("new_password").is_none());
    stop.cancel();
    admin_handle.abort();
    dav_handle.abort();
}

#[tokio::test]
async fn receiver_shares_management_port_and_honors_enablement() {
    let (_dir, db) = database().await;
    let stop = CancellationToken::new();
    let (url, handle) =
        serve(management_router(db.clone()).merge(receiver_router(db.clone(), stop.clone()))).await;
    let client = reqwest::Client::new();
    assert!(
        client
            .get(format!("{url}/api/sites/webdav-sync"))
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    assert_eq!(
        client
            .request(reqwest::Method::OPTIONS, format!("{url}/dav/ptd/"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(
        client
            .request(reqwest::Method::OPTIONS, format!("{url}/dav/ptd/"))
            .basic_auth("ptd", Some(PASSWORD))
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    let response = client.put(format!("{url}/api/sites/webdav-sync")).json(&json!({"enabled":false,"username":"ptd","existing_policy":"update","auto_create":true})).send().await.unwrap();
    assert!(response.status().is_success());
    assert_eq!(
        client
            .get(format!("{url}/dav/ptd/"))
            .basic_auth("ptd", Some(PASSWORD))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    stop.cancel();
    handle.abort();
}

#[tokio::test]
async fn manual_import_works_without_webdav_and_reuses_duplicate_rules() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let (_dir, db) = database().await;
    configure(&db, false, "skip").await;
    let (url, handle) = serve(management_router(db.clone())).await;
    let client = reqwest::Client::new();
    let address = format!("{url}/api/sites/ptd-import");
    for (body, policy, count) in [
        (
            archive(
                &cookies("manual-one"),
                chrono::Utc::now().timestamp_millis(),
            ),
            "update",
            "created",
        ),
        (cookies("manual-one"), "update", "unchanged"),
        (cookies("manual-two"), "skip", "skipped"),
        (cookies("manual-two"), "update", "updated"),
    ] {
        let response = client.post(&address).json(&json!({"content_base64":STANDARD.encode(body),"existing_policy":policy,"auto_create":true})).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let report: Value = response.json().await.unwrap();
        assert_eq!(report[count], 1);
        assert!(!report.to_string().contains("manual-one"));
    }
    for content in [STANDARD.encode(b"invalid json"), "invalid base64!".into()] {
        assert_eq!(
            client
                .post(&address)
                .json(
                    &json!({"content_base64":content,"existing_policy":"update","auto_create":true})
                )
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    db.dav(|conn| {
        assert!(!store::config(conn)?.enabled);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sites", [], |r| r.get(0))
            .map_err(sql)?;
        assert_eq!(count, 1);
        let resources: i64 = conn
            .query_row("SELECT COUNT(*) FROM webdav_sync_resources", [], |r| {
                r.get(0)
            })
            .map_err(sql)?;
        assert_eq!(resources, 0);
        Ok(())
    })
    .await
    .unwrap();
    handle.abort();
}

#[tokio::test]
async fn manual_import_rolls_back_on_database_failure() {
    let (_dir, db) = database().await;
    db.dav(|conn| {
        conn.execute_batch("CREATE TRIGGER reject_manual BEFORE UPDATE OF status ON webdav_sync_jobs WHEN NEW.status='done' BEGIN SELECT RAISE(ABORT,'fixture'); END;").map_err(sql)?;
        assert!(store::import_manual(conn, import::parse(&cookies("synthetic")).unwrap(), "update".into(), true).is_err());
        let sites: i64 = conn.query_row("SELECT COUNT(*) FROM sites", [], |r| r.get(0)).map_err(sql)?;
        let jobs: i64 = conn.query_row("SELECT COUNT(*) FROM webdav_sync_jobs", [], |r| r.get(0)).map_err(sql)?;
        assert_eq!((sites,jobs),(0,0));
        Ok(())
    }).await.unwrap();
}
