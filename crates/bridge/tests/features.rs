use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use codex_mobile_bridge::{
    config::{new_thread_defaults, MessageModel},
    context,
    desktop::Desktop,
    history::History,
    images::{attachment_paths, context_prompt, Images},
    notifications::Notifications,
    paths::Paths,
};
use serde_json::{json, Value};
use std::{fs, sync::Arc};

#[test]
fn existing_images_keep_their_paths_and_native_attachment_format() {
    let temp = tempfile::tempdir().unwrap();
    let images = Images {
        directory: temp.path().join("images"),
    };
    fs::create_dir_all(&images.directory).unwrap();
    let bytes = STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+j0KkAAAAASUVORK5CYII=").unwrap();
    let id = "0123456789abcdef0123456789abcdef.png";
    // The existing Node service writes one image and this adjacent metadata file.
    let metadata = json!({"id":id,"name":"截图 [1].png","size":bytes.len(),"type":"image/png"});
    fs::write(images.directory.join(id), &bytes).unwrap();
    fs::write(
        images.directory.join(format!("{id}.json")),
        metadata.to_string(),
    )
    .unwrap();
    let attachments = images.attachments(&[id.into()]).unwrap();
    let image_path = images.directory.join(id).to_string_lossy().into_owned();
    assert_eq!(
        attachments,
        vec![json!({"id":id,"src":image_path,"localPath":image_path,"filename":"截图 [1].png"})]
    );
    assert_eq!(fs::read(images.directory.join(id)).unwrap(), bytes);
    let prompt = context_prompt("查看这个报错", &attachments);
    assert!(prompt.contains("截图 &#91;1&#93;.png"));
    assert!(prompt.ends_with("我的请求：\n查看这个报错"));
    let item = json!({"type":"userMessage","content":[{"type":"text","text":prompt}]});
    let paths = attachment_paths(&item, &images.directory);
    assert_eq!(paths.len(), 1);
    assert_eq!(fs::read(&paths[0]).unwrap(), bytes);
    let unrelated = json!({"type":"userMessage","content":[{"type":"text","text":format!("[file](<{}>)",temp.path().join("unrelated.png").display())}]});
    assert!(attachment_paths(&unrelated, &images.directory).is_empty());
    for name in ["create_thread", "send_message_to_thread"] {
        let delegated = json!({"type":"functionCallOutput","namespace":"codex_app","name":name,
            "output":format!("<codex_delegation>\n<source_thread_id>source</source_thread_id>\n<input>\n{}\n</input>\n</codex_delegation>", prompt.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;"))});
        assert_eq!(attachment_paths(&delegated, &images.directory), paths);
        let mut unrelated = delegated.clone();
        unrelated["namespace"] = json!("another_tool");
        assert!(attachment_paths(&unrelated, &images.directory).is_empty());
    }
    assert_eq!(context_prompt("文字", &[]), "文字");
    assert!(context_prompt("", &attachments).ends_with("请查看这些图片。"));
    let saved = images.save(&bytes, "diagram%20%5B2%5D.png").unwrap();
    assert_eq!(saved["name"], "diagram [2].png");
    assert_eq!(saved["type"], "image/png");
    assert_eq!(
        images.read(saved["id"].as_str().unwrap()).unwrap()["name"],
        saved["name"]
    );
    assert_eq!(
        images.save(b"<svg></svg>", "image.svg").unwrap_err().status,
        400
    );
}

#[test]
fn message_model_omits_defaults_and_rejects_unsupported_input_before_sending() {
    let efforts = vec!["low".to_string(), "high".to_string()];
    let mut args = json!({"prompt":"message","threadId":"chat"});
    let defaults = MessageModel::parse(&json!({}), &efforts).unwrap();
    assert!(defaults.is_empty());
    defaults.apply(&mut args);
    assert_eq!(args, json!({"prompt":"message","threadId":"chat"}));
    MessageModel::parse(&json!({"thinking":"high"}), &efforts)
        .unwrap()
        .apply(&mut args);
    assert_eq!(args["thinking"], "high");
    assert!(args.get("model").is_none());
    let choice = MessageModel::parse(
        &json!({"model":"model/selected","thinking":"low"}),
        &efforts,
    )
    .unwrap();
    choice.apply(&mut args);
    assert_eq!(args["model"], "model/selected");
    assert_eq!(args["thinking"], "low");
    for invalid in [
        json!({"model":null}),
        json!({"model":"bad model"}),
        json!({"thinking":"ultra"}),
        json!({"thinking":2}),
    ] {
        let error = MessageModel::parse(&invalid, &efforts).unwrap_err();
        assert_eq!(error.status, 400);
        assert_eq!(error.outcome.as_deref(), Some("not-dispatched"));
    }
}

#[tokio::test]
async fn saved_notifications_reuse_keys_and_follow_repaired_sessions() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().to_path_buf();
    let fresh = Notifications::open(directory.clone()).unwrap();
    let public_key = fresh.public_key();
    let saved_keys = fs::read(directory.join("push-keys.json")).unwrap();
    drop(fresh);
    let subscription = json!({"endpoint":"https://web.push.apple.com/isolated-fixture","keys":{"p256dh":public_key,"auth":URL_SAFE_NO_PAD.encode([7u8;16])}});
    let expires = codex_mobile_bridge::now_ms() + 60_000;
    fs::write(directory.join("push-subscriptions.json"), json!([
        {"subscription":subscription,"owner":"old-session","expiresAt":expires,"origin":"https://phone.example","threadIds":["old-chat"],"lastError":""},
        {"subscription":{"endpoint":"https://web.push.apple.com/expired"},"owner":"expired-session","expiresAt":1}
    ]).to_string()).unwrap();
    let notifications = Notifications::open(directory.clone()).unwrap();
    assert_eq!(notifications.public_key(), public_key);
    assert_eq!(
        notifications
            .status(&subscription, "new-session", expires)
            .await
            .unwrap(),
        json!({"enabled":true,"error":""})
    );
    notifications
        .remove_for_owner("old-session", None)
        .await
        .unwrap();
    assert!(notifications.has_subscriptions().await);
    let disk: Value =
        serde_json::from_slice(&fs::read(directory.join("push-subscriptions.json")).unwrap())
            .unwrap();
    assert_eq!(disk.as_array().unwrap().len(), 1);
    assert_eq!(disk[0]["owner"], "new-session");
    assert!(disk[0].get("threadIds").is_none());
    assert!(disk[0]["enabledAt"].as_i64().unwrap() > 0);
    let mut mismatched = subscription.clone();
    mismatched["keys"]["auth"] = json!(URL_SAFE_NO_PAD.encode([8u8; 16]));
    assert!(notifications
        .status(&mismatched, "other", expires)
        .await
        .is_err());
    notifications
        .remove_for_owner("new-session", None)
        .await
        .unwrap();
    assert!(!notifications.has_subscriptions().await);
    assert!(
        !Notifications::open(directory.clone())
            .unwrap()
            .has_subscriptions()
            .await
    );
    assert_eq!(
        fs::read(directory.join("push-keys.json")).unwrap(),
        saved_keys
    );
}

#[tokio::test]
async fn project_defaults_and_context_preserve_configured_sources() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("codex");
    let project = temp.path().join("project");
    let nested = project.join("nested");
    for directory in [
        &home,
        &project.join(".git"),
        &project.join(".codex"),
        &nested.join(".codex"),
    ] {
        fs::create_dir_all(directory).unwrap();
    }
    let project_key = serde_json::to_string(&project.to_string_lossy()).unwrap();
    let config = format!("model = 'desktop-default'\nmodel_reasoning_effort = 'high'\n[projects.{project_key}]\ntrust_level = 'trusted'\n[plugins.'pdf@test']\nenabled = true\n[plugins.'disabled@test']\nenabled = false\n");
    fs::write(home.join("config.toml"), &config).unwrap();
    fs::write(
        project.join(".codex/config.toml"),
        "model = 'project-default'\n",
    )
    .unwrap();
    fs::write(
        nested.join(".codex/config.toml"),
        "model_reasoning_effort = 'low'\n",
    )
    .unwrap();
    assert_eq!(
        new_thread_defaults(&home, Some(&nested)).unwrap(),
        json!({"model":"project-default","effort":"low"})
    );
    assert_eq!(
        new_thread_defaults(&home, None).unwrap(),
        json!({"model":"desktop-default","effort":"high"})
    );
    fs::write(
        home.join("config.toml"),
        config.replace("'trusted'", "'untrusted'"),
    )
    .unwrap();
    assert_eq!(
        new_thread_defaults(&home, Some(&nested)).unwrap(),
        json!({"model":"desktop-default","effort":"high"})
    );
    fs::write(home.join("config.toml"), "model = 'desktop-default'\n").unwrap();
    assert_eq!(
        new_thread_defaults(&home, Some(&nested))
            .unwrap_err()
            .status,
        503
    );
    fs::write(home.join("config.toml"), &config).unwrap();

    let plugin = home.join("plugins/cache/test/pdf/1.0.0/.codex-plugin");
    fs::create_dir_all(&plugin).unwrap();
    fs::write(plugin.join("plugin.json"), json!({"name":"pdf","interface":{"displayName":"PDF","shortDescription":"阅读文档"},"hooks":{"Stop":[]}}).to_string()).unwrap();
    let files = temp.path().join("files");
    fs::create_dir_all(files.join("node_modules")).unwrap();
    fs::write(files.join("README.md"), "Isolated source").unwrap();
    fs::write(files.join("node_modules/README.md"), "Generated dependency").unwrap();
    let desktop = Desktop::new(
        Paths {
            codex_home: home.clone(),
            data_home: temp.path().join("data"),
            runtime_home: temp.path().join("runtime"),
            web_root: temp.path().join("web"),
        },
        Arc::new(History::new(home.clone())),
    );
    let plugins = context::read(&desktop, None, None, "pdf").await;
    assert_eq!(
        plugins["plugins"],
        json!([{"id":"pdf@test","kind":"plugin","label":"PDF","description":"阅读文档","mention":"[PDF](plugin://pdf@test)"}])
    );
    let catalog = context::read(&desktop, Some(&files), None, "readme").await;
    assert_eq!(catalog["files"].as_array().unwrap().len(), 1);
    assert_eq!(
        catalog["files"][0]["mention"],
        format!(
            "[README.md](<{}>)",
            files.join("README.md").to_string_lossy().replace('\\', "/")
        )
    );
    fs::write(home.join("hooks.json"), r#"{"Stop":[{"hooks":[]}]}"#).unwrap();
    let hooks = context::hooks(&home, None);
    assert_eq!(hooks["mode"], "read-only");
    assert_eq!(hooks["sources"].as_array().unwrap().len(), 2);
    assert!(hooks["issues"].as_array().unwrap().is_empty());
    fs::write(home.join("hooks.json"), "invalid json").unwrap();
    assert_eq!(
        context::hooks(&home, None)["issues"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        fs::read_to_string(home.join("config.toml")).unwrap(),
        config
    );
}
