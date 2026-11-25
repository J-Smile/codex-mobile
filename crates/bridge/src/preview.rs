use crate::{images::markdown_paths, BridgeError, Result};
use axum::{
    body::Body,
    http::{Request, Response, StatusCode},
};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::HashMap, path::Path, sync::LazyLock};
use tower_http::services::ServeFile;

#[derive(Clone, Copy, Deserialize, Hash, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Html,
    Markdown,
    Text,
    Image,
    Audio,
    Video,
    Disabled,
}

static FORMATS: LazyLock<HashMap<String, Kind>> = LazyLock::new(|| {
    let groups: HashMap<Kind, Vec<String>> =
        serde_json::from_str(include_str!("../../../shared/preview-formats.json"))
            .expect("预览格式表无效");
    groups
        .into_iter()
        .flat_map(|(kind, formats)| formats.into_iter().map(move |extension| (extension, kind)))
        .collect()
});

fn kind(path: &Path) -> Option<Kind> {
    let extension = path.extension().or_else(|| path.file_name())?.to_str()?;
    FORMATS
        .get(&extension.trim_start_matches('.').to_ascii_lowercase())
        .copied()
}

pub(crate) fn check_attachment_type(path: &Path) -> Result<()> {
    if kind(path) == Some(Kind::Disabled) {
        return Err(BridgeError::new(415, "此类型不支持预览或下载。"));
    }
    Ok(())
}

fn referenced(item: &Value, reference: &str) -> bool {
    match item["type"].as_str() {
        Some("agentMessage") => {
            let text = item["text"].as_str().unwrap_or("");
            markdown_paths(text).any(|path| path == reference)
                || text.split("visualize").skip(1).any(|part| {
                    part.split_once('').is_some_and(|(value, _)| {
                        serde_json::from_str::<Value>(value)
                            .is_ok_and(|link| link["path"] == reference)
                    })
                })
        }
        Some("fileChange") => item["changes"].as_array().is_some_and(|changes| {
            changes.iter().any(|change| {
                change["path"] == reference
                    && change["kind"] != "delete"
                    && change["kind"]["type"] != "delete"
            })
        }),
        _ => false,
    }
}

fn thread_output(path: &Path, home: &Path, id: &str) -> bool {
    path.strip_prefix(home.join("attachments").join(id)).is_ok()
        || path
            .strip_prefix(home.join("visualizations"))
            .is_ok_and(|tail| {
                let parts: Vec<_> = tail.iter().collect();
                parts.len() >= 5
                    && parts[3] == id
                    && parts[..3].iter().zip([4, 2, 2]).all(|(part, length)| {
                        let bytes = part.as_encoded_bytes();
                        bytes.len() == length && bytes.iter().all(u8::is_ascii_digit)
                    })
            })
}

pub(crate) async fn response(
    codex_home: &Path,
    metadata: &Value,
    id: &str,
    item: &Value,
    reference: &str,
    request: Request<Body>,
) -> Result<Response<Body>> {
    if !referenced(item, reference) {
        return Err(BridgeError::new(403, "聊天中没有引用这个附件。"));
    }
    let invalid = || BridgeError::invalid("只能预览本机文件。");
    let missing = || BridgeError::new(404, "文件已移走或无法读取。");
    static LINE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r":\d+(?::\d+)?$").unwrap());
    let reference = LINE.replace(reference.split(['?', '#']).next().unwrap_or_default(), "");
    let reference = reference.as_ref();
    let mut path = if reference
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file:///"))
    {
        url::Url::parse(reference)
            .ok()
            .and_then(|url| url.to_file_path().ok())
            .ok_or_else(invalid)?
    } else {
        let decoded = percent_encoding::percent_decode_str(reference)
            .decode_utf8()
            .map_err(|_| invalid())?;
        // Markdown 绝对路径在 Windows 上也可写成 /C:/…。
        let local = if cfg!(windows)
            && matches!(decoded.as_bytes(), [b'/', letter, b':', b'/' | b'\\', ..] if letter.is_ascii_alphabetic())
        {
            &decoded[1..]
        } else {
            decoded.as_ref()
        };
        let drive = cfg!(windows)
            && matches!(local.as_bytes(), [letter, b':', b'/' | b'\\', ..] if letter.is_ascii_alphabetic());
        if local.starts_with(['/', '\\']) && local[1..].starts_with(['/', '\\'])
            || local.contains(':') && (!drive || local[2..].contains(':'))
        {
            return Err(invalid());
        }
        Path::new(local).to_path_buf()
    };
    let local = path.to_string_lossy();
    if local.starts_with("//")
        || local.starts_with("\\\\")
        || (cfg!(windows) && local.get(2..).is_some_and(|tail| tail.contains(':')))
    {
        return Err(invalid());
    }
    let requested_kind =
        kind(&path).ok_or_else(|| BridgeError::invalid("此文件类型暂不支持预览。"))?;
    check_attachment_type(&path)?;
    let root = match metadata["cwd"]
        .as_str()
        .map(Path::new)
        .filter(|path| path.is_absolute())
    {
        Some(root) => tokio::fs::canonicalize(root).await.ok(),
        None => None,
    };
    if !path.is_absolute() {
        path = root
            .as_ref()
            .ok_or_else(|| BridgeError::invalid("这个聊天没有可用的本机工作目录。"))?
            .join(path);
    }
    let file = tokio::fs::canonicalize(path).await.map_err(|_| missing())?;
    let allowed = if root.as_ref().is_some_and(|root| file.starts_with(root)) {
        true
    } else {
        tokio::fs::canonicalize(codex_home)
            .await
            .is_ok_and(|home| thread_output(&file, &home, id))
    };
    if !allowed || kind(&file) != Some(requested_kind) {
        return Err(BridgeError::new(
            403,
            "只能预览当前聊天项目或产物目录中的文件。",
        ));
    }
    let stat = tokio::fs::metadata(&file).await.map_err(|_| missing())?;
    if !stat.is_file() {
        return Err(missing());
    }
    let maximum = match requested_kind {
        Kind::Html | Kind::Markdown | Kind::Text => 5,
        Kind::Audio | Kind::Video => 512,
        _ => 64,
    };
    if stat.len() > maximum * 1024 * 1024 {
        return Err(BridgeError::new(
            413,
            format!("文件超过 {maximum} MB，无法预览。"),
        ));
    }
    let mime = match requested_kind {
        Kind::Audio | Kind::Video | Kind::Image => {
            mime_guess::from_path(&file).first_or_octet_stream()
        }
        _ => mime_guess::mime::TEXT_PLAIN_UTF_8,
    };
    let mut result = ServeFile::new_with_mime(file, &mime)
        .try_call(request)
        .await
        .map_err(|_| missing())?
        .map(Body::new);
    if result.status() == StatusCode::NOT_FOUND {
        return Err(missing());
    }
    let headers = result.headers_mut();
    headers.insert("cache-control", "private, no-store".parse().unwrap());
    headers.insert("x-content-type-options", "nosniff".parse().unwrap());
    headers.insert(
        "content-security-policy",
        "sandbox; default-src 'none'".parse().unwrap(),
    );
    Ok(result)
}
