use super::*;

#[test]
fn chrome_bash_json_cookie_and_transport_headers() {
    let parsed=parse("curl 'https://example.test/api?x=1&x=2' \\\n -H 'accept: application/json' \\\n -H 'content-type: application/json' -H 'Connection: keep-alive' \\\n -b 'session=abc; id=42' --data-raw '{\"text\":\"你好\"}' --compressed").unwrap();
    assert_eq!(parsed.http.method, "POST");
    assert_eq!(parsed.http.url, "https://example.test/api?x=1&x=2");
    assert_eq!(parsed.http.body_type, "json");
    assert_eq!(parsed.http.body, "{\"text\":\"你好\"}");
    assert!(matches!(parsed.http.auth,Some(Auth::Cookie{ref value})if value=="session=abc; id=42"));
    assert_eq!(parsed.http.headers.len(), 2);
    assert!(parsed.notes.iter().any(|n| n.contains("Host")));
}
#[test]
fn bash_quotes_ansi_escapes_and_literal_commands_are_never_executed() {
    let words=bash_words(r#"curl 'https://example.test' -H 'X-Text: it'\''s literal' --data-raw $'line\n\u4f60\u597d\t\x41\101'"#).unwrap();
    assert_eq!(words[3], "X-Text: it's literal");
    assert_eq!(words[5], "line\n你好\tAA");
    let parsed = parse(
        r#"curl 'https://example.test' --data-raw '$(touch /tmp/never-created); $TOKEN `whoami`'"#,
    )
    .unwrap();
    assert!(parsed.http.body.contains("$(touch"));
    for command in [
        "curl https://example.test; whoami",
        "curl $(echo https://example.test)",
        "curl \"https://example.test/$TOKEN\"",
        "curl https://example.test > out",
        "curl 'https://example.test",
        "curl 'https://example.test' --data-raw $'\\0'",
    ] {
        assert!(parse(command).is_err(), "{command}");
    }
}
#[test]
fn urlencoded_preserves_raw_bytes_and_get_moves_data_into_query() {
    let parsed =
        parse("curl 'https://example.test' -d 'a=1&a=2' --data-urlencode 'q=a+b &中'").unwrap();
    assert_eq!(parsed.http.body, "a=1&a=2&q=a%2Bb+%26%E4%B8%AD");
    assert_eq!(
        parsed.http.content_type,
        "application/x-www-form-urlencoded"
    );
    let parsed =
        parse("curl -GsS 'https://example.test?existing=1' --data-urlencode 'q=a+b &中'").unwrap();
    assert_eq!(parsed.http.method, "GET");
    assert_eq!(parsed.http.body_type, "none");
    assert_eq!(
        parsed.http.url,
        "https://example.test/?existing=1&q=a%2Bb+%26%E4%B8%AD"
    );
}
#[test]
fn basic_bearer_custom_headers_and_options() {
    let parsed=parse("curl --url=https://example.test -XPUT -u 'name:pass:word' -H'X-Custom: value' -m45 -L --json '{\"ok\":true}'").unwrap();
    assert_eq!(parsed.http.method, "PUT");
    assert!(parsed.http.follow_redirects);
    assert_eq!(parsed.http.timeout_seconds, 45);
    assert!(matches!(parsed.http.auth,Some(Auth::Basic{ref password,..})if password=="pass:word"));
    assert_eq!(parsed.http.headers.len(), 3);
    let parsed =
        parse("curl 'https://example.test' -H 'Authorization: Bearer abc' -b 'session=xyz'")
            .unwrap();
    assert!(matches!(parsed.http.auth,Some(Auth::Bearer{ref token})if token=="abc"));
    assert_eq!(parsed.http.headers[0].name, "Cookie");
    assert!(parse("curl 'https://example.test' -u a:b -H 'Authorization: Basic xyz'").is_err());
}
#[test]
fn form_text_empty_values_and_unsupported_file_references() {
    let parsed = parse(
        "curl 'https://example.test' -F 'tag=one' --form-string 'tag=@literal;keep' -F 'empty='",
    )
    .unwrap();
    assert_eq!(parsed.http.body_type, "multipart");
    assert_eq!(parsed.http.form_fields.len(), 3);
    assert_eq!(parsed.http.form_fields[1].value, "@literal;keep");
    for command in [
        "curl 'https://example.test' -F 'file=@/tmp/file'",
        "curl 'https://example.test' --data-binary @file",
        "curl 'https://example.test' --data-urlencode name@file",
        "curl 'https://example.test' -b cookies.txt",
        "curl 'https://example.test' --proxy http://proxy",
        "curl 'https://example.test' -k",
        "curl 'https://example.test' --next 'https://other.test'",
        "curl 'https://example.test' --max-time 0",
        "curl 'https://example.test' --request",
    ] {
        assert!(parse(command).is_err(), "{command}");
    }
    assert_eq!(
        parse("curl 'https://example.test' --data-raw '@literal'")
            .unwrap()
            .http
            .body,
        "@literal"
    );
}
#[tokio::test]
async fn imported_config_builds_the_expected_request() {
    let parsed =
        parse("curl 'https://example.test/api' -u 'user:pass' -d 'a=1&a=2' -H 'X-Test: yes'")
            .unwrap();
    let request = parsed.http.build_request(&reqwest::Client::new()).unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.headers()["authorization"], "Basic dXNlcjpwYXNz");
    assert_eq!(
        request.headers()["content-type"],
        "application/x-www-form-urlencoded"
    );
    assert_eq!(request.body().unwrap().as_bytes().unwrap(), b"a=1&a=2");
}

#[test]
fn url_globbing_requires_explicit_literal_mode() {
    assert!(parse("curl 'https://example.com/{a,b}'").is_err());
    assert!(parse("curl 'https://example.com/?a[0]=1'").is_err());
    assert!(parse("curl -g 'https://example.com/?a[0]=1'").is_ok());
    assert!(parse("curl 'http://[::1]/health'").is_ok());
}
