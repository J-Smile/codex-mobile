use crate::{config::read_toml, desktop::Desktop, paths::normalized, BridgeError, Result};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

struct Plugin {
    id: String,
    directory: PathBuf,
    manifest: Value,
}
fn enabled_plugins(home: &Path) -> Result<Vec<Plugin>> {
    let config = read_toml(&home.join("config.toml"))?;
    let mut plugins = vec![];
    for (id, settings) in config
        .as_ref()
        .and_then(|c| c.get("plugins"))
        .and_then(toml::Value::as_table)
        .into_iter()
        .flatten()
    {
        if settings.get("enabled").and_then(toml::Value::as_bool) == Some(false) {
            continue;
        }
        let Some((name, registry)) = id.split_once('@') else {
            continue;
        };
        if [name, registry].iter().any(|s| {
            s.is_empty()
                || !s
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
        }) {
            continue;
        }
        let directory = home.join("plugins/cache").join(registry).join(name);
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        let mut versions: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|s| !s.starts_with('.'))
            .collect();
        versions.sort_by(
            |a, b| match (semver::Version::parse(a), semver::Version::parse(b)) {
                (Ok(a), Ok(b)) => b.cmp(&a),
                _ => b.cmp(a),
            },
        );
        if let Some(version) = versions.first() {
            let directory = directory.join(version);
            let manifest = match fs::read(directory.join(".codex-plugin/plugin.json")) {
                Ok(bytes) => serde_json::from_slice(&bytes)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            plugins.push(Plugin {
                id: id.clone(),
                directory,
                manifest,
            });
        }
    }
    Ok(plugins)
}
fn label(value: &str) -> String {
    value
        .replace(['\r', '\n'], " ")
        .replace('\\', r"\\")
        .replace('[', r"\[")
        .replace(']', r"\]")
}
fn matching(value: &str, query: &str) -> bool {
    value.to_lowercase().contains(&query.to_lowercase())
}
fn plugins(home: &Path, query: &str) -> Result<Vec<Value>> {
    let mut entries = vec![];
    for plugin in enabled_plugins(home)? {
        let manifest = plugin.manifest;
        let name = manifest["interface"]["displayName"]
            .as_str()
            .or_else(|| manifest["name"].as_str())
            .unwrap_or("");
        let description = manifest["interface"]["shortDescription"]
            .as_str()
            .or_else(|| {
                manifest["description"]
                    .as_str()
                    .and_then(|s| s.lines().next())
            })
            .unwrap_or("");
        if !name.is_empty() && matching(&format!("{name} {description} {}", plugin.id), query) {
            entries.push(json!({"id":plugin.id,"kind":"plugin","label":name,"description":description,"mention":format!("[{}](plugin://{})",label(name),plugin.id)}));
        }
    }
    entries.sort_by(|a, b| a["label"].as_str().cmp(&b["label"].as_str()));
    Ok(entries)
}
async fn files(cwd: Option<&Path>, query: &str) -> Result<Vec<Value>> {
    let Some(cwd) = cwd.filter(|_| !query.trim().is_empty()) else {
        return Ok(vec![]);
    };
    let mut command = tokio::process::Command::new("git");
    command
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .current_dir(cwd);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let output = command.output().await;
    let mut names = match output {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>(),
        Ok(output) if output.status.code() != Some(128) => {
            return Err(BridgeError::unavailable("无法搜索项目文件。"))
        }
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
        _ => {
            let root = cwd.to_path_buf();
            tokio::task::spawn_blocking(move || {
                walkdir::WalkDir::new(&root)
                    .into_iter()
                    .filter_entry(|e| {
                        !e.file_type().is_dir()
                            || ![
                                ".git",
                                "node_modules",
                                ".local",
                                "dist",
                                ".next",
                                ".venv",
                                "target",
                            ]
                            .contains(&e.file_name().to_string_lossy().as_ref())
                    })
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_type().is_file())
                    .filter_map(|e| {
                        e.path()
                            .strip_prefix(&root)
                            .ok()
                            .map(|p| p.to_string_lossy().into_owned())
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .map_err(|e| BridgeError::unavailable(e.to_string()))?
        }
    };
    names.sort();
    names.dedup();
    names.retain(|s| matching(s, query));
    names.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
    names.truncate(40);
    Ok(names.iter().map(|name|{let absolute=cwd.join(name).to_string_lossy().replace('\\',"/");let filename=Path::new(name).file_name().unwrap_or_default().to_string_lossy();json!({"id":absolute,"kind":"file","label":filename,"description":name.replace('\\',"/"),"mention":format!("[{}](<{}>)",label(&filename),absolute.replace('>',"%3E"))})}).collect())
}
fn browser_inventory(result: Value) -> Result<Value> {
    if result["isError"] == true {
        let exited = result["_meta"]["codex/telemetry"]["error_code"] == "kernel_exited";
        return Err(BridgeError::unavailable(if exited {
            "电脑上的浏览器读取运行环境已退出，请在电脑恢复后重试。"
        } else {
            "官方工具未能读取此对话的浏览器，请在电脑检查后重试。"
        }));
    }
    if let Some(inventory) = result
        .get("structuredContent")
        .filter(|value| value["browsers"].is_array())
    {
        return Ok(inventory.clone());
    }
    // getState emits its documentation and inventory as separate content
    // blocks. Only the inventory belongs in the mobile context picker.
    for content in result["content"].as_array().into_iter().flatten() {
        if let Some(text) = content["text"]
            .as_str()
            .filter(|_| content["type"] == "text")
        {
            if let Ok(inventory) = serde_json::from_str::<Value>(text) {
                if inventory["browsers"].is_array() {
                    return Ok(inventory);
                }
            }
        }
    }
    Err(BridgeError::unavailable(
        "官方工具未返回有效的浏览器清单，请在电脑检查后重试。",
    ))
}
async fn browser_tabs(desktop: &Arc<Desktop>, thread: Option<&str>, query: &str) -> Result<Value> {
    let Some(thread) = thread else {
        return Ok(json!({"browserTabs":[],"browserMessage":"先打开一条对话以选择浏览器标签页。"}));
    };
    let result = desktop.browser_inventory(thread).await.map_err(|error| {
        tracing::warn!("官方浏览器清单请求失败：{}", error.message);
        BridgeError::unavailable("官方浏览器工具暂时不可用，请在电脑检查后重试。")
    })?;
    let inventory = browser_inventory(result)?;
    let mut entries = std::collections::BTreeMap::new();
    for browser in inventory["browsers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|browser| browser["metadata"]["codexSessionId"] == thread)
    {
        for tab in browser["tabs"].as_array().into_iter().flatten() {
            let Some(tab_id) = tab["providerTabId"].as_str().filter(|id| !id.is_empty()) else {
                continue;
            };
            let Some(url) = tab["url"].as_str().filter(|u| !u.is_empty()) else {
                continue;
            };
            let title = tab["title"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(url);
            if !matching(&format!("{title} {url}"), query) {
                continue;
            }
            let params = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs([
                    ("mention", "tab-v1"),
                    ("browserId", thread),
                    ("tabId", tab_id),
                    ("title", title),
                    ("url", url),
                ])
                .finish();
            let id = format!("{thread}:{tab_id}");
            entries.insert(id.clone(),json!({"id":id,"kind":"browser","label":title,"description":url,"mention":format!("[{}](plugin://browser@openai-bundled?{params})",label(title))}));
        }
    }
    let mut result = json!({"browserTabs":entries.values().take(20).collect::<Vec<_>>()});
    if entries.is_empty() {
        if inventory["errors"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .any(|error| error.starts_with("Browsers:"))
        {
            return Err(BridgeError::unavailable(
                "官方工具报告浏览器清单读取失败，请在电脑检查后重试。",
            ));
        }
        result["browserMessage"] = json!("当前对话没有匹配的浏览器标签页。");
    }
    Ok(result)
}
// 普通引用只搜索本机项目，不调用插件或浏览器控制入口。
pub async fn references(cwd: Option<&Path>, query: &str) -> Value {
    let mut result = json!({"mode":"references","plugins":[],"browserTabs":[],"files":[]});
    match files(cwd, query).await {
        Ok(files) => result["files"] = json!(files),
        Err(_) => {
            result["fileMessage"] = json!("无法搜索这个项目的文件，请检查项目目录是否可访问。");
        }
    }
    result
}
pub async fn read(
    desktop: &Arc<Desktop>,
    cwd: Option<&Path>,
    thread: Option<&str>,
    query: &str,
) -> Value {
    let home = &desktop.paths.codex_home;
    let plugin_result = plugins(home, query);
    let (mut result, browser_result) =
        tokio::join!(references(cwd, query), browser_tabs(desktop, thread, query));
    let browser = browser_result
        .unwrap_or_else(|error| json!({"browserTabs":[],"browserMessage":format!("暂时无法读取此对话的浏览器标签页。{}",error.message)}));
    result
        .as_object_mut()
        .unwrap()
        .extend(browser.as_object().unwrap().clone());
    result["mode"] = json!("context");
    match plugin_result {
        Ok(v) => result["plugins"] = json!(v),
        Err(_) => {
            result["plugins"] = json!([]);
            result["pluginMessage"] = json!("无法读取电脑上的插件配置，请稍后重试。");
        }
    }
    result
}
pub fn hooks(home: &Path, cwd: Option<&Path>) -> Value {
    let mut sources = vec![];
    let mut issues = vec![];
    let mut files = vec![("用户 Hooks".to_string(), home.join("hooks.json"), true)];
    if let Some(cwd) = cwd {
        files.push(("项目 Hooks".into(), cwd.join(".codex/hooks.json"), true));
    }
    match enabled_plugins(home){Err(_)=>issues.push(json!({"path":home.join("config.toml"),"message":"无法读取已启用插件的配置，请在电脑检查插件。"})),Ok(plugins)=>{
        for plugin in plugins{let Some(hooks)=plugin.manifest.get("hooks")else{continue};let name=plugin.manifest["interface"]["displayName"].as_str().or_else(||plugin.manifest["name"].as_str()).unwrap_or(&plugin.id);
            if let Some(file)=hooks.as_str(){let path=plugin.directory.join(file);let clean=path.components().fold(PathBuf::new(),|mut path,component|{if component==std::path::Component::ParentDir{path.pop();}else if component!=std::path::Component::CurDir{path.push(component.as_os_str());}path});
                if !normalized(&clean).starts_with(&(normalized(&plugin.directory)+std::path::MAIN_SEPARATOR_STR)){issues.push(json!({"path":plugin.directory.join(".codex-plugin/plugin.json"),"message":"Hooks 路径超出插件目录，请在电脑检查插件。"}));continue;}
                files.push((name.into(),clean,false));
            }else if hooks.is_object(){sources.push(json!({"label":name,"path":plugin.directory.join(".codex-plugin/plugin.json"),"config":hooks}));}
            else{issues.push(json!({"path":plugin.directory.join(".codex-plugin/plugin.json"),"message":"Hooks 配置应为 JSON 对象，请在电脑检查文件。"}));}
        }
    }}
    for (label, path, optional) in files {
        match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                Ok(config) if config.is_object() => {
                    sources.push(json!({"label":label,"path":path,"config":config}))
                }
                _ => issues.push(
                    json!({"path":path,"message":"配置不是有效的 JSON 对象，请在电脑检查文件。"}),
                ),
            },
            Err(e) if optional && e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                issues.push(json!({"path":path,"message":"无法读取配置，请在电脑检查文件。"}))
            }
        }
    }
    json!({"mode":"read-only","pendingReview":null,"sources":sources,"issues":issues})
}
