use crate::{paths::write_private, BridgeError, Result};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

pub const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
pub fn valid_id(id: &str) -> bool {
    let Some((stem, extension)) = id.rsplit_once('.') else {
        return false;
    };
    stem.len() == 32
        && stem
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && matches!(extension, "png" | "jpg" | "webp" | "gif")
}
pub fn image_ids(value: Option<&Value>) -> Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(vec![]);
    };
    let ids = value
        .as_array()
        .ok_or_else(|| BridgeError::invalid("图片附件无效，请重新选择。").before_send())?;
    let mut known = HashSet::new();
    if ids.len() > 20
        || ids.iter().any(|v| {
            !v.as_str()
                .is_some_and(|id| valid_id(id) && known.insert(id))
        })
    {
        return Err(BridgeError::invalid("图片附件无效，请重新选择。").before_send());
    }
    Ok(ids
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect())
}
pub fn format(bytes: &[u8]) -> Result<(&'static str, &'static str)> {
    if bytes.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10]) {
        Ok(("png", "image/png"))
    } else if bytes.starts_with(&[255, 216, 255]) {
        Ok(("jpg", "image/jpeg"))
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Ok(("gif", "image/gif"))
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Ok(("webp", "image/webp"))
    } else {
        Err(BridgeError::invalid("请选择 PNG、JPEG、WebP 或 GIF 图片。").before_send())
    }
}

#[derive(Clone)]
pub struct Images {
    pub directory: PathBuf,
}
impl Images {
    pub fn save(&self, bytes: &[u8], filename: &str) -> Result<Value> {
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(BridgeError::new(413, "每张图片不能超过 20 MB。").before_send());
        }
        let (extension, mime) = format(bytes)?;
        let id = format!("{}.{}", hex::encode(rand::random::<[u8; 16]>()), extension);
        let name = percent_encoding::percent_decode_str(filename)
            .decode_utf8()
            .unwrap_or_default()
            .chars()
            .map(|c| {
                if c == '/' || c == '\\' || c.is_control() {
                    '_'
                } else {
                    c
                }
            })
            .take(180)
            .collect::<String>();
        let name = if name.trim().is_empty() {
            format!("图片.{extension}")
        } else {
            name.trim().to_string()
        };
        let metadata = json!({"id":id,"name":name,"size":bytes.len(),"type":mime});
        std::fs::create_dir_all(&self.directory)?;
        write_private(&self.directory.join(&id), bytes)?;
        write_private(
            &self.directory.join(format!("{id}.json")),
            &serde_json::to_vec(&metadata)?,
        )?;
        let mut value = metadata;
        value["url"] = json!(format!("/api/images/{id}"));
        Ok(value)
    }
    pub fn read(&self, id: &str) -> Result<Value> {
        if !valid_id(id) {
            return Err(BridgeError::new(404, "图片附件不存在。"));
        }
        let path = self.directory.join(id);
        let bytes = std::fs::read(self.directory.join(format!("{id}.json")))
            .map_err(|_| BridgeError::new(404, "图片附件已不可用，请重新选择。").before_send())?;
        let stat = std::fs::metadata(&path)
            .map_err(|_| BridgeError::new(404, "图片附件已不可用，请重新选择。").before_send())?;
        let mut value: Value = serde_json::from_slice(&bytes)?;
        value["path"] = json!(path);
        value["size"] = json!(stat.len());
        value["url"] = json!(format!("/api/images/{id}"));
        Ok(value)
    }
    pub fn attachments(&self, ids: &[String]) -> Result<Vec<Value>> {
        ids.iter().map(|id|{let image=self.read(id)?;Ok(json!({"id":id,"src":image["path"],"localPath":image["path"],"filename":image["name"]}))}).collect()
    }
}
pub(crate) fn markdown_paths(text: &str) -> impl Iterator<Item = &str> {
    static LINKS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"!?\[[^\]]*\]\((?:<([^>]+)>|([^\)\s]+))(?:\s+"[^"]*")?\)"#).unwrap()
    });
    LINKS
        .captures_iter(text)
        .filter_map(|cap| cap.get(1).or_else(|| cap.get(2)).map(|path| path.as_str()))
}

pub fn attachment_paths(item: &Value, image_directory: &Path) -> Vec<String> {
    let mut result = vec![];
    let mut add = |value: &str| {
        if Path::new(value).is_absolute() {
            result.push(value.to_string());
        }
    };
    if item["type"] == "imageView" {
        if let Some(path) = item["path"].as_str() {
            add(path);
        }
    }
    let content = if item["type"] == "userMessage" {
        &item["content"]
    } else {
        &item["input"]
    };
    for part in content.as_array().into_iter().flatten() {
        if part["type"] == "localImage" {
            if let Some(path) = part["path"].as_str() {
                add(path);
            }
        }
    }
    if item["type"] == "agentMessage" {
        for path in markdown_paths(item["text"].as_str().unwrap_or("")) {
            add(path);
        }
    }
    let user_text = content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|part| {
            item["type"] == "userMessage" && (part["type"] == "text" || part["type"] == "inputText")
        })
        .filter_map(|part| part["text"].as_str());
    let delegated = crate::state::chat_creation_input(item);
    for text in user_text.chain(delegated.as_deref()) {
        for path in markdown_paths(text) {
            let path = Path::new(path);
            if path.parent().is_some_and(|p| {
                crate::paths::normalized(p) == crate::paths::normalized(image_directory)
            }) && path
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(valid_id)
            {
                add(path.to_str().unwrap());
            }
        }
    }
    result
}
