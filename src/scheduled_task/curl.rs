//! A deliberately non-executing Bash/cURL subset. Never spawn a shell, expand
//! variables, read local files, or resolve remote content while importing.
use super::http::{Auth, FormField, HttpConfig, Pair};
use serde::Serialize;

#[derive(Serialize)]
pub struct CurlImport {
    pub http: HttpConfig,
    pub notes: Vec<String>,
}

fn bash_words(input: &str) -> Result<Vec<String>, String> {
    if input.len() > 524288 {
        return Err("cURL 命令最大 512 KiB".into());
    }
    let mut chars = input.trim().chars().peekable();
    let mut words = Vec::new();
    let mut word = String::new();
    let mut active = false;
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => match chars.next() {
                Some('\n') => {}
                Some('\r') if chars.peek() == Some(&'\n') => {
                    chars.next();
                }
                Some(c) => {
                    word.push(c);
                    active = true;
                }
                None => return Err("命令结尾的反斜杠缺少内容".into()),
            },
            '\'' | '"' => {
                active = true;
                let quote = ch;
                let mut closed = false;
                while let Some(c) = chars.next() {
                    if c == quote {
                        closed = true;
                        break;
                    }
                    if quote == '"' && c == '\\' {
                        let next = chars.next().ok_or("双引号中的转义不完整")?;
                        match next {
                            '\n' => {}
                            '$' | '`' | '"' | '\\' => word.push(next),
                            _ => {
                                word.push('\\');
                                word.push(next);
                            }
                        }
                    } else {
                        if quote == '"' && (c == '`' || c == '$') {
                            return Err("不支持 Bash 变量或命令替换，请粘贴展开后的值（字面量 $ 可放在单引号内）".into());
                        }
                        word.push(c);
                    }
                }
                if !closed {
                    return Err("引号未闭合，请检查复制的命令是否完整".into());
                }
            }
            '$' if chars.peek() == Some(&'\'') => {
                chars.next();
                active = true;
                let mut closed = false;
                while let Some(c) = chars.next() {
                    if c == '\'' {
                        closed = true;
                        break;
                    }
                    if c != '\\' {
                        word.push(c);
                        continue;
                    }
                    let escape = chars.next().ok_or("Bash 转义不完整")?;
                    match escape {
                        'n' => word.push('\n'),
                        'r' => word.push('\r'),
                        't' => word.push('\t'),
                        'a' => word.push('\x07'),
                        'b' => word.push('\x08'),
                        'f' => word.push('\x0c'),
                        'v' => word.push('\x0b'),
                        'e' | 'E' => word.push('\x1b'),
                        '\\' | '\'' | '"' => word.push(escape),
                        'u' | 'U' | 'x' | '0'..='7' => {
                            let (radix, max, mut digits) = match escape {
                                'u' => (16, 4, String::new()),
                                'U' => (16, 8, String::new()),
                                'x' => (16, 2, String::new()),
                                _ => (8, 3, escape.to_string()),
                            };
                            while digits.len() < max
                                && chars.peek().is_some_and(|c| c.is_digit(radix))
                            {
                                digits.push(chars.next().unwrap());
                            }
                            let value = u32::from_str_radix(&digits, radix)
                                .map_err(|_| "Bash 数字转义无效")?;
                            if matches!(escape, 'x' | '0'..='7') && value > 127 {
                                return Err("不支持非 ASCII 字节转义，请粘贴 UTF-8 文本或手动上传二进制文件".into());
                            }
                            word.push(char::from_u32(value).ok_or("Bash Unicode 转义无效")?);
                        }
                        _ => {
                            word.push('\\');
                            word.push(escape);
                        }
                    }
                }
                if !closed {
                    return Err("Bash $'…' 引号未闭合".into());
                }
            }
            '$' | '`' => return Err("不支持 Bash 变量或命令替换，请粘贴展开后的值".into()),
            '|' | '&' | ';' | '<' | '>' | '(' | ')' => {
                return Err(
                    "只支持单条 cURL 命令；请移除管道、重定向或命令连接符，并用引号包裹 URL".into(),
                );
            }
            '#' if !active => {
                while chars.peek().is_some_and(|c| *c != '\n') {
                    chars.next();
                }
            }
            c if c.is_whitespace() => {
                if active {
                    words.push(std::mem::take(&mut word));
                    active = false;
                }
            }
            c => {
                if matches!(c, '*' | '?' | '[' | ']' | '{' | '}' | '~') {
                    return Err("请用单引号包裹 URL 和包含 Bash 通配符的参数".into());
                }
                word.push(c);
                active = true;
            }
        }
    }
    if active {
        words.push(word);
    }
    if words.iter().any(|w| w.contains('\0')) {
        return Err("命令包含 NUL 字符，请使用文件方式配置二进制内容".into());
    }
    Ok(words)
}

fn note(notes: &mut Vec<String>, text: &str) {
    if !notes.iter().any(|n| n == text) {
        notes.push(text.into());
    }
}
fn set_header(headers: &mut Vec<Pair>, name: &str, value: String) {
    headers.retain(|h| !h.name.eq_ignore_ascii_case(name));
    headers.push(Pair {
        name: name.into(),
        value,
    });
}
fn encode(value: &str) -> String {
    reqwest::Url::parse_with_params("http://localhost", [("", value)])
        .unwrap()
        .query()
        .unwrap()
        .trim_start_matches('=')
        .to_string()
}

pub fn parse(input: &str) -> Result<CurlImport, String> {
    let words = bash_words(input)?;
    if !words
        .first()
        .is_some_and(|v| v == "curl" || v == "curl.exe" || v == "/usr/bin/curl")
    {
        return Err("请粘贴以 curl 开头的 Bash 命令".into());
    }
    let mut config = HttpConfig {
        send_via_browser: false,
        use_global_proxy: false,
        browser_use_global_proxy: false,
        method: "GET".into(),
        url: String::new(),
        query: vec![],
        headers: vec![],
        bearer_token: String::new(),
        auth: Some(Auth::None),
        body: String::new(),
        body_type: "none".into(),
        form_fields: vec![],
        binary: None,
        content_type: String::new(),
        timeout_seconds: 30,
        follow_redirects: false,
        expected_status: None,
    };
    let mut notes = vec![
        "发送方式、代理和成功状态码沿用当前面板；超时默认 30 秒，命令中的 --max-time 可覆盖。"
            .into(),
    ];
    let mut urls = Vec::new();
    let mut data = Vec::new();
    let mut json_mode = false;
    let mut form = false;
    let mut explicit_method = None;
    let mut head = false;
    let mut get = false;
    let mut user = None;
    let mut cookie = None;
    let mut compressed = false;
    let mut globoff = false;
    let mut i = 1;
    let mut positional = false;
    while i < words.len() {
        let arg = &words[i];
        i += 1;
        if arg == "--" {
            positional = true;
            continue;
        }
        if positional || !arg.starts_with('-') {
            urls.push(arg.clone());
            continue;
        }
        let (option, inline) = if arg.starts_with("--") {
            arg.split_once('=')
                .map(|(k, v)| (k.to_string(), Some(v.to_string())))
                .unwrap_or((arg.clone(), None))
        } else {
            let short = arg.chars().nth(1).ok_or("不支持从标准输入读取")?;
            if "XHdbuAeFm".contains(short) {
                (
                    format!("-{short}"),
                    (arg.len() > 2).then(|| arg[2..].to_string()),
                )
            } else if arg[1..].chars().all(|c| "sSvLiIgG".contains(c)) {
                for c in arg[1..].chars() {
                    match c {
                        'L' => config.follow_redirects = true,
                        'I' => head = true,
                        'G' => get = true,
                        'g' => globoff = true,
                        _ => note(
                            &mut notes,
                            "已忽略终端输出选项（silent/show-error/verbose），不影响请求配置。",
                        ),
                    }
                }
                continue;
            } else {
                (arg.clone(), None)
            }
        };
        let takes_value = matches!(
            option.as_str(),
            "--url"
                | "-X"
                | "--request"
                | "-H"
                | "--header"
                | "-d"
                | "--data"
                | "--data-ascii"
                | "--data-raw"
                | "--data-binary"
                | "--data-urlencode"
                | "--json"
                | "-b"
                | "--cookie"
                | "-u"
                | "--user"
                | "-A"
                | "--user-agent"
                | "-e"
                | "--referer"
                | "-F"
                | "--form"
                | "--form-string"
                | "-m"
                | "--max-time"
                | "--oauth2-bearer"
        );
        let value = if takes_value {
            match inline {
                Some(v) => v,
                None => {
                    let v = words.get(i).ok_or("cURL 选项缺少参数值")?.clone();
                    i += 1;
                    v
                }
            }
        } else {
            if inline.is_some() {
                return Err("该 cURL 开关不接受参数值".into());
            }
            String::new()
        };
        match option.as_str() {
            "--url" => urls.push(value),
            "-X" | "--request" => explicit_method = Some(value),
            "-H" | "--header" => {
                if value.starts_with('@') {
                    return Err("不读取本地请求头文件，请将 -H 内容直接粘贴到命令中".into());
                }
                let (name, value) = value
                    .split_once(':')
                    .ok_or("请求头需要使用 '名称: 值' 格式")?;
                let name = name.trim();
                let value = value.trim_start().to_string();
                if name.is_empty() || value.is_empty() {
                    return Err("不支持空请求头或移除默认请求头的语法，请在配置面板中调整".into());
                }
                if [
                    "host",
                    "content-length",
                    "connection",
                    "transfer-encoding",
                    "accept-encoding",
                ]
                .iter()
                .any(|v| name.eq_ignore_ascii_case(v))
                {
                    note(
                        &mut notes,
                        "Host、Content-Length、Connection、Transfer-Encoding、Accept-Encoding 由发送客户端自动管理，已略过对应请求头。",
                    );
                    continue;
                }
                config.headers.push(Pair {
                    name: name.into(),
                    value,
                });
            }
            "-d" | "--data" | "--data-ascii" | "--data-binary" | "--data-raw"
            | "--data-urlencode" | "--json" => {
                if form {
                    return Err("不能同时导入 --form 和 --data 请求体".into());
                }
                if value.starts_with('@') && option != "--data-raw" {
                    return Err("不读取本地文件或标准输入，请导入后在请求体面板选择文件".into());
                }
                if option == "--json" {
                    if !data.is_empty() && !json_mode {
                        return Err("请勿混合 --json 与 --data".into());
                    }
                    json_mode = true;
                } else if json_mode {
                    return Err("请勿混合 --json 与 --data".into());
                }
                let value = if option == "--data-urlencode" {
                    if let Some((name, value)) = value.split_once('=') {
                        if name.is_empty() {
                            encode(value)
                        } else {
                            format!("{name}={}", encode(value))
                        }
                    } else if value.contains('@') {
                        return Err("--data-urlencode 不支持读取本地文件".into());
                    } else {
                        encode(&value)
                    }
                } else {
                    value
                };
                data.push(value);
            }
            "-b" | "--cookie" => {
                if !value.contains('=') {
                    return Err("-b/--cookie 需要直接提供 Cookie，不能导入 Cookie 文件".into());
                }
                cookie = Some(value);
            }
            "-u" | "--user" => {
                let (username, password) = value
                    .split_once(':')
                    .ok_or("Basic Auth 需要 username:password，不能交互读取密码")?;
                user = Some(Auth::Basic {
                    username: username.into(),
                    password: password.into(),
                });
            }
            "--oauth2-bearer" => user = Some(Auth::Bearer { token: value }),
            "-A" | "--user-agent" => set_header(&mut config.headers, "User-Agent", value),
            "-e" | "--referer" => {
                if value.ends_with(";auto") {
                    return Err("不支持自动 Referer，请提供固定 Referer 值".into());
                }
                set_header(&mut config.headers, "Referer", value);
            }
            "-F" | "--form" | "--form-string" => {
                if !data.is_empty() {
                    return Err("不能同时导入 --form 和 --data 请求体".into());
                }
                form = true;
                let (name, value) = value
                    .split_once('=')
                    .ok_or("表单字段需要 name=value 格式")?;
                if option != "--form-string"
                    && (value.starts_with(['@', '<']) || value.contains(';'))
                {
                    return Err(
                        "文件或高级 multipart 字段不能直接导入；请导入文本字段后在面板中选择文件"
                            .into(),
                    );
                }
                config.form_fields.push(FormField {
                    name: name.into(),
                    value: value.into(),
                    file: None,
                });
            }
            "-m" | "--max-time" => {
                config.timeout_seconds = value
                    .parse::<u64>()
                    .map_err(|_| "超时需要 1–300 的整数秒")?;
            }
            "--location" => config.follow_redirects = true,
            "--no-location" => config.follow_redirects = false,
            "--head" => head = true,
            "--get" => get = true,
            "--compressed" => compressed = true,
            "--silent" | "--show-error" | "--verbose" | "--no-progress-meter" => note(
                &mut notes,
                "已忽略终端输出选项（silent/show-error/verbose），不影响请求配置。",
            ),
            "--globoff" => globoff = true,
            "--basic" => {}
            "--insecure" | "-k" => {
                return Err("不支持跳过 TLS 证书校验（--insecure/-k），请移除该选项后重试".into());
            }
            _ => {
                return Err(format!(
                    "不支持的 cURL 选项：{}。请移除该选项，或在导入后通过配置面板设置。",
                    option
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                        .take(40)
                        .collect::<String>()
                ));
            }
        }
    }
    if urls.len() != 1 {
        return Err("一次只能导入一个 HTTP 请求，请提供且仅提供一个 URL".into());
    }
    let url = reqwest::Url::parse(&urls[0]).map_err(|_| "URL 无效，请包含 http:// 或 https://")?;
    if !globoff
        && (urls[0].contains(['{', '}'])
            || url.path().contains(['[', ']'])
            || url.query().is_some_and(|q| q.contains(['[', ']'])))
    {
        return Err("不支持 cURL URL 通配展开；如需按字面量发送，请添加 --globoff（-g）".into());
    }
    if url.fragment().is_some() {
        note(
            &mut notes,
            "URL 的 #fragment 不参与 HTTP 请求，导入后仍保留在地址中。",
        );
    }
    config.url = urls.remove(0);
    if get && form {
        return Err("--get 不支持 multipart 表单".into());
    }
    if head && (!data.is_empty() || form) {
        return Err("HEAD 与请求体不能同时导入".into());
    }
    let has_data = !data.is_empty();
    config.method = explicit_method.unwrap_or_else(|| {
        if head {
            "HEAD"
        } else if get {
            "GET"
        } else if has_data || form {
            "POST"
        } else {
            "GET"
        }
        .into()
    });
    if head && config.method != "HEAD" {
        return Err("-I/--head 与指定方法冲突".into());
    }
    if get && has_data {
        let query = data.join("&");
        let mut url = reqwest::Url::parse(&config.url).unwrap();
        let query = match url.query() {
            Some(old) if !old.is_empty() => format!("{old}&{query}"),
            _ => query,
        };
        url.set_query(Some(&query));
        config.url = url.to_string();
    } else if form {
        config.body_type = "multipart".into();
    } else if has_data {
        config.body = data.join(if json_mode { "" } else { "&" });
        if json_mode {
            if !config
                .headers
                .iter()
                .any(|h| h.name.eq_ignore_ascii_case("content-type"))
            {
                set_header(
                    &mut config.headers,
                    "Content-Type",
                    "application/json".into(),
                );
            }
            if !config
                .headers
                .iter()
                .any(|h| h.name.eq_ignore_ascii_case("accept"))
            {
                set_header(&mut config.headers, "Accept", "application/json".into());
            }
        }
        let content_type = config
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("content-type"))
            .map(|h| h.value.clone())
            .unwrap_or("application/x-www-form-urlencoded".into());
        let mime = content_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        config.body_type = match mime.as_str() {
            "application/json"
                if serde_json::from_str::<serde_json::Value>(&config.body).is_ok() =>
            {
                "json"
            }
            "text/plain" => "text",
            "application/xml" | "text/xml" => "xml",
            "text/html" => "html",
            _ => "raw",
        }
        .into();
        config.content_type = content_type;
        if mime == "application/x-www-form-urlencoded" {
            note(
                &mut notes,
                "URL 编码正文以自定义原文导入，保留转义、重复字段和原始字节。可在请求体面板继续编辑。",
            );
        }
    }
    let auth_headers = config
        .headers
        .iter()
        .filter(|h| h.name.eq_ignore_ascii_case("authorization"))
        .count();
    if user.is_some() && auth_headers > 0 {
        return Err("命令同时设置认证选项和 Authorization 请求头，请只保留一种".into());
    }
    if cookie.is_some()
        && config
            .headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("cookie"))
    {
        return Err("命令同时设置 --cookie 和 Cookie 请求头，请只保留一种".into());
    }
    if let Some(cookie) = cookie {
        set_header(&mut config.headers, "Cookie", cookie);
    }
    if let Some(auth) = user {
        config.auth = Some(auth);
    } else if auth_headers == 1 {
        let index = config
            .headers
            .iter()
            .position(|h| h.name.eq_ignore_ascii_case("authorization"))
            .unwrap();
        let value = &config.headers[index].value;
        if let Some(token) = value.strip_prefix("Bearer ") {
            config.auth = Some(Auth::Bearer {
                token: token.into(),
            });
            config.headers.remove(index);
        }
    }
    if matches!(config.auth, Some(Auth::None))
        && config
            .headers
            .iter()
            .filter(|h| h.name.eq_ignore_ascii_case("cookie"))
            .count()
            == 1
    {
        let index = config
            .headers
            .iter()
            .position(|h| h.name.eq_ignore_ascii_case("cookie"))
            .unwrap();
        let cookie = config.headers.remove(index);
        config.auth = Some(Auth::Cookie {
            value: cookie.value,
        });
    }
    if compressed {
        note(
            &mut notes,
            "--compressed 使用客户端自动协商压缩和解压，不固定 Accept-Encoding。",
        );
    }
    config.validate()?;
    Ok(CurlImport {
        http: config,
        notes,
    })
}

#[cfg(test)]
mod tests;
