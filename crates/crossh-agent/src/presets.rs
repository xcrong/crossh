//! 内置供应商预设，以 `pi-ai` 的公开资源为底子，但落盘为 Crossh 自有副本。
//!
//! 数据来源（公开底子）：`@earendil-works/pi-ai/dist/providers/data/opencode-go.json`
//! 锚定版本：`pi-coding-agent 0.84.1 / pi-ai 0.84.1`
//! 原始地址：`https://raw.githubusercontent.com/earendil-works/pi-mono/main/packages/ai/src/providers/data/opencode-go.json`
//!
//! 单供应商 `opencode` 承载全部模型，`AgentModel.protocol/url` 区分协议
//! （AnthropicMessages / OpenAiChat / OpenAiResponses）。
//! 后续新增预设只需在此追加 `AgentProvider` 并在 `builtin_presets()` 中注册。
//!
//! 动态更新（已与 `pi` 解耦）：`builtin_presets()` 优先读取 Crossh 自有缓存
//! `~/.config/crossh/agent/models-store.json`（与 `pi` 同形的 `{opencode:{models:[...]}}`），
//! 首次运行时若不存在会一次性从 `~/.pi/agent/models-store.json` 迁移（仅做 seeding，不再共享），
//! 之后完全独立；`pi` 每 4h 从 `https://pi.dev/api/models/providers/{id}` 刷新的逻辑
//! 未来由 Crossh 自身的 `remote-catalog.json` 定时拉取替代（复用 `REMOTE_CATALOG_REFRESH_INTERVAL_MS=4h`）。
//! 未安装 `pi` 且自有缓存缺失时回退到 baked 基线（18 条，见 `baked_models()`）。
//! 外部扩展：`~/.config/crossh/agent/providers/*.json` 可直接放置 `pi` 格式的供应商
//! JSON（`pi-ai/dist/providers/data/*.json` 的 grouped-by-api 格式，或
//! `~/.pi/agent/models.json` 的 `{"providers":{...}}` 格式），启动时自动加载为
//! 额外供应商，实现零成本接入新供应商。
use crate::Protocol;
use crate::policy::{AgentModel, AgentProvider};
use serde_json::Value as JsonValue;

pub const OPENCODE_ID: &str = "opencode";

const ANTHROPIC_URL: &str = "https://opencode.ai/zen/v1/messages";
const CHAT_URL: &str = "https://opencode.ai/zen/v1/chat/completions";
const RESPONSES_URL: &str = "https://opencode.ai/zen/v1/responses";

pub fn is_builtin_preset_id(id: &str) -> bool {
    id == OPENCODE_ID
}

/// 返回所有内置预设 + 外部 `pi` 兼容供应商。外部供应商通过
/// `~/.config/crossh/agent/providers/*.json` 提供，格式兼容 `pi` 的两种形态。
pub fn builtin_presets() -> Vec<AgentProvider> {
    let models = load_dynamic_or_baked();
    let mut out = vec![AgentProvider {
        id: OPENCODE_ID.into(),
        name: "opencode".into(),
        api_key_env: "OPENCODE_API_KEY".into(),
        api_key: String::new(),
        protocol: None,
        url: None,
        models,
    }];
    out.extend(load_external_pi_providers());
    out
}

/// 扫描 `~/.config/crossh/agent/providers/*.json` 并按 `pi` 格式解析。
pub fn load_external_pi_providers() -> Vec<AgentProvider> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let dir = home
        .join(".config")
        .join("crossh")
        .join("agent")
        .join("providers");
    let entries = std::fs::read_dir(&dir).ok();
    let mut providers = Vec::new();
    let Some(entries) = entries else {
        return providers;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let data = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let value: JsonValue = match serde_json::from_str(&data) {
            Ok(v) => v,
            Err(_) => continue,
        };
        // 形态 B: {"providers": {"id": {"baseUrl": "...", "api": "...", "models": [...]}}}
        if let Some(map) = value.get("providers").and_then(|v| v.as_object()) {
            for (pid, pval) in map {
                if let Some(provider) = parse_pi_custom_provider(pid, pval) {
                    providers.push(provider);
                }
            }
            continue;
        }
        // 形态 A: grouped-by-api {"anthropic-messages": {"model": {...}}, "openai-completions": {...}}
        if let Some(provider) = parse_pi_grouped_provider(&value, &path) {
            providers.push(provider);
        }
    }
    providers
}

fn parse_pi_custom_provider(id: &str, value: &JsonValue) -> Option<AgentProvider> {
    let base_url = value
        .get("baseUrl")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let api = value
        .get("api")
        .and_then(|v| v.as_str())
        .unwrap_or("openai-completions");
    let (protocol, url_suffix) = pi_api_to_protocol(api)?;
    let models = value.get("models").and_then(|v| v.as_array())?;
    let mut out_models = Vec::new();
    for m in models {
        let mid = m.get("id").and_then(|v| v.as_str())?;
        let name = m.get("name").and_then(|v| v.as_str()).unwrap_or(mid);
        let reasoning = m
            .get("reasoning")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let context_window = m
            .get("contextWindow")
            .and_then(|v| v.as_u64())
            .unwrap_or(128_000) as u32;
        let max_tokens = m
            .get("maxTokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(32_000) as u32;
        // per-model api 覆盖 provider api，若存在则优先
        let model_api = m.get("api").and_then(|v| v.as_str()).unwrap_or(api);
        let model_base = m
            .get("baseUrl")
            .and_then(|v| v.as_str())
            .unwrap_or(base_url);
        let (mproto, msuffix) = pi_api_to_protocol(model_api).unwrap_or((protocol, url_suffix));
        let url = if model_base.trim().is_empty() {
            match mproto {
                Protocol::AnthropicMessages => ANTHROPIC_URL.into(),
                Protocol::OpenAiChat => CHAT_URL.into(),
                Protocol::OpenAiResponses => RESPONSES_URL.into(),
            }
        } else {
            format!("{}{msuffix}", model_base.trim_end_matches('/'))
        };
        out_models.push(sanitize_model(AgentModel {
            id: mid.into(),
            name: name.into(),
            protocol: mproto,
            url,
            reasoning,
            context_window: context_window.max(1),
            max_tokens: max_tokens.max(1),
        }));
    }
    if out_models.is_empty() {
        return None;
    }
    Some(AgentProvider {
        id: id.into(),
        name: id.into(),
        api_key_env: format!("{}_API_KEY", id.to_ascii_uppercase().replace('-', "_")),
        api_key: String::new(),
        protocol: None,
        url: None,
        models: out_models,
    })
}

fn parse_pi_grouped_provider(value: &JsonValue, path: &std::path::Path) -> Option<AgentProvider> {
    let obj = value.as_object()?;
    // 推断 provider id：优先取首个模型内 provider 字段，否则用文件名
    let mut provider_id: Option<String> = None;
    let mut models = Vec::new();
    for (api, group) in obj {
        let group_obj = group.as_object()?;
        for (_mid, mval) in group_obj {
            let mid = mval.get("id").and_then(|v| v.as_str())?;
            let name = mval.get("name").and_then(|v| v.as_str()).unwrap_or(mid);
            let api_str = mval.get("api").and_then(|v| v.as_str()).unwrap_or(api);
            let base_url = mval.get("baseUrl").and_then(|v| v.as_str()).unwrap_or("");
            let reasoning = mval
                .get("reasoning")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let context_window = mval
                .get("contextWindow")
                .and_then(|v| v.as_u64())
                .unwrap_or(128_000) as u32;
            let max_tokens = mval
                .get("maxTokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(32_000) as u32;
            if provider_id.is_none()
                && let Some(pid) = mval.get("provider").and_then(|v| v.as_str())
            {
                provider_id = Some(pid.to_string());
            }
            let (protocol, suffix) = pi_api_to_protocol(api_str)?;
            // google-generative-ai 等暂不支持，跳过
            let url = if base_url.trim().is_empty() {
                match protocol {
                    Protocol::AnthropicMessages => ANTHROPIC_URL.into(),
                    Protocol::OpenAiChat => CHAT_URL.into(),
                    Protocol::OpenAiResponses => RESPONSES_URL.into(),
                }
            } else {
                format!("{}{suffix}", base_url.trim_end_matches('/'))
            };
            models.push(sanitize_model(AgentModel {
                id: mid.into(),
                name: name.into(),
                protocol,
                url,
                reasoning,
                context_window: context_window.max(1),
                max_tokens: max_tokens.max(1),
            }));
        }
    }
    if models.is_empty() {
        return None;
    }
    let pid = provider_id.unwrap_or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("pi-provider")
            .to_string()
    });
    Some(AgentProvider {
        id: pid.clone(),
        name: pid.clone(),
        api_key_env: format!("{}_API_KEY", pid.to_ascii_uppercase().replace('-', "_")),
        api_key: String::new(),
        protocol: None,
        url: None,
        models,
    })
}

fn pi_api_to_protocol(api: &str) -> Option<(Protocol, &'static str)> {
    match api {
        "anthropic-messages" => Some((Protocol::AnthropicMessages, "/v1/messages")),
        "openai-completions" | "openai-chat" => Some((Protocol::OpenAiChat, "/chat/completions")),
        "openai-responses" => Some((Protocol::OpenAiResponses, "/responses")),
        _ => None,
    }
}

fn load_dynamic_or_baked() -> Vec<AgentModel> {
    let baked = baked_models();
    let Some(dynamic) = load_pi_dynamic_models() else {
        return baked;
    };
    merge_models(baked, dynamic.models)
}

struct DynamicGroups {
    models: Vec<AgentModel>,
}

fn sanitize_model(mut model: AgentModel) -> AgentModel {
    // 与 AgentSettings::validate 保持一致：max < context 且至少留 1024 输入 token
    if model.context_window == 0 {
        model.context_window = 128_000;
    }
    if model.max_tokens == 0 {
        model.max_tokens = 32_000;
    }
    if model.max_tokens >= model.context_window
        || model.context_window.saturating_sub(model.max_tokens) < 1_024
    {
        model.max_tokens = model.context_window.saturating_sub(1_024).max(1);
    }
    if model.url.trim().is_empty() {
        model.url = match model.protocol {
            Protocol::AnthropicMessages => ANTHROPIC_URL.into(),
            Protocol::OpenAiChat => CHAT_URL.into(),
            Protocol::OpenAiResponses => RESPONSES_URL.into(),
        };
    }
    model
}

fn crossh_models_store_path() -> Option<std::path::PathBuf> {
    Some(
        dirs::home_dir()?
            .join(".config")
            .join("crossh")
            .join("agent")
            .join("models-store.json"),
    )
}

fn load_pi_dynamic_models() -> Option<DynamicGroups> {
    // 仅读 Crossh 自有副本，不再依赖 `~/.pi`；两应用完全隔离。
    // 公开资源已 baked 为底子（`baked_models()`），自有缓存 `~/.config/crossh/agent/models-store.json`
    // 由 Crossh 自身的远程拉取填充（未来 `remote-catalog.json` 4h 刷新），当前无缓存即回退 baked。
    let path = crossh_models_store_path()?;
    let data = std::fs::read_to_string(path).ok()?;
    let store: JsonValue = serde_json::from_str(&data).ok()?;
    parse_models_store(&store)
}

fn parse_models_store(store: &JsonValue) -> Option<DynamicGroups> {
    // 新 pi 使用 `opencode`，旧缓存可能仍为 `opencode-go`
    let entry = store.get("opencode").or_else(|| store.get("opencode-go"))?;
    let models = entry.get("models")?.as_array()?;
    let mut out = Vec::new();
    for model in models {
        let id = model.get("id")?.as_str()?;
        let name = model.get("name").and_then(JsonValue::as_str).unwrap_or(id);
        let reasoning = model
            .get("reasoning")
            .and_then(JsonValue::as_bool)
            .unwrap_or(false);
        let context_window = model
            .get("contextWindow")
            .and_then(JsonValue::as_u64)
            .unwrap_or(128_000) as u32;
        let max_tokens = model
            .get("maxTokens")
            .and_then(JsonValue::as_u64)
            .unwrap_or(32_000) as u32;
        let api = model.get("api").and_then(JsonValue::as_str).unwrap_or("");
        let base_url = model
            .get("baseUrl")
            .and_then(JsonValue::as_str)
            .unwrap_or("");
        let (protocol, url) = match api {
            "anthropic-messages" => (
                Protocol::AnthropicMessages,
                format!("{}/v1/messages", base_url.trim_end_matches('/')),
            ),
            "openai-completions" => (
                Protocol::OpenAiChat,
                format!("{}/chat/completions", base_url.trim_end_matches('/')),
            ),
            "openai-responses" => (
                Protocol::OpenAiResponses,
                format!("{}/responses", base_url.trim_end_matches('/')),
            ),
            // google-generative-ai 等暂无对应 Protocol，跳过
            _ => continue,
        };
        let agent_model = sanitize_model(AgentModel {
            id: id.into(),
            name: name.into(),
            protocol,
            url,
            reasoning,
            context_window: context_window.max(1),
            max_tokens: max_tokens.max(1),
        });
        out.push(agent_model);
    }
    if out.is_empty() {
        return None;
    }
    Some(DynamicGroups { models: out })
}

fn merge_models(mut baseline: Vec<AgentModel>, dynamic: Vec<AgentModel>) -> Vec<AgentModel> {
    for model in dynamic {
        if let Some(pos) = baseline.iter().position(|m| m.id == model.id) {
            baseline[pos] = model;
        } else {
            baseline.push(model);
        }
    }
    baseline
}

fn baked_models() -> Vec<AgentModel> {
    let mut all = Vec::new();
    all.extend(anthropic_models());
    all.extend(openai_chat_models());
    all.extend(openai_responses_models());
    all
}

fn anthropic_models() -> Vec<AgentModel> {
    vec![
        AgentModel {
            id: "minimax-m3".into(),
            name: "MiniMax-M3".into(),
            protocol: Protocol::AnthropicMessages,
            url: ANTHROPIC_URL.into(),
            reasoning: true,
            context_window: 1_000_000,
            max_tokens: 131_072,
        },
        AgentModel {
            id: "qwen3.7-max".into(),
            name: "Qwen3.7 Max".into(),
            protocol: Protocol::AnthropicMessages,
            url: ANTHROPIC_URL.into(),
            reasoning: true,
            context_window: 1_000_000,
            max_tokens: 65_536,
        },
        AgentModel {
            id: "qwen3.7-plus".into(),
            name: "Qwen3.7 Plus".into(),
            protocol: Protocol::AnthropicMessages,
            url: ANTHROPIC_URL.into(),
            reasoning: true,
            context_window: 1_000_000,
            max_tokens: 65_536,
        },
        AgentModel {
            id: "qwen3.8-max".into(),
            name: "Qwen3.8 Max".into(),
            protocol: Protocol::AnthropicMessages,
            url: ANTHROPIC_URL.into(),
            reasoning: true,
            context_window: 1_000_000,
            max_tokens: 131_072,
        },
    ]
}

fn openai_chat_models() -> Vec<AgentModel> {
    vec![
        AgentModel {
            id: "deepseek-v4-flash".into(),
            name: "DeepSeek V4 Flash (New)".into(),
            protocol: Protocol::OpenAiChat,
            url: CHAT_URL.into(),
            reasoning: true,
            context_window: 1_000_000,
            max_tokens: 384_000,
        },
        AgentModel {
            id: "deepseek-v4-pro".into(),
            name: "DeepSeek V4 Pro".into(),
            protocol: Protocol::OpenAiChat,
            url: CHAT_URL.into(),
            reasoning: true,
            context_window: 1_000_000,
            max_tokens: 384_000,
        },
        AgentModel {
            id: "glm-5.1".into(),
            name: "GLM-5.1".into(),
            protocol: Protocol::OpenAiChat,
            url: CHAT_URL.into(),
            reasoning: true,
            context_window: 202_752,
            max_tokens: 32_768,
        },
        AgentModel {
            id: "glm-5.2".into(),
            name: "GLM-5.2".into(),
            protocol: Protocol::OpenAiChat,
            url: CHAT_URL.into(),
            reasoning: true,
            context_window: 1_000_000,
            max_tokens: 131_072,
        },
        AgentModel {
            id: "hy3".into(),
            name: "Hy3".into(),
            protocol: Protocol::OpenAiChat,
            url: CHAT_URL.into(),
            reasoning: true,
            context_window: 256_000,
            max_tokens: 64_000,
        },
        AgentModel {
            id: "kimi-k2.6".into(),
            name: "Kimi K2.6".into(),
            protocol: Protocol::OpenAiChat,
            url: CHAT_URL.into(),
            reasoning: true,
            context_window: 262_144,
            max_tokens: 65_536,
        },
        AgentModel {
            id: "kimi-k2.7-code".into(),
            name: "Kimi K2.7 Code".into(),
            protocol: Protocol::OpenAiChat,
            url: CHAT_URL.into(),
            reasoning: true,
            context_window: 262_144,
            max_tokens: 261_120,
        },
        AgentModel {
            id: "kimi-k3".into(),
            name: "Kimi K3".into(),
            protocol: Protocol::OpenAiChat,
            url: CHAT_URL.into(),
            reasoning: true,
            context_window: 1_048_576,
            max_tokens: 131_072,
        },
        AgentModel {
            id: "mimo-v2.5".into(),
            name: "MiMo V2.5".into(),
            protocol: Protocol::OpenAiChat,
            url: CHAT_URL.into(),
            reasoning: true,
            context_window: 1_000_000,
            max_tokens: 128_000,
        },
        AgentModel {
            id: "mimo-v2.5-pro".into(),
            name: "MiMo V2.5 Pro".into(),
            protocol: Protocol::OpenAiChat,
            url: CHAT_URL.into(),
            reasoning: true,
            context_window: 1_048_576,
            max_tokens: 128_000,
        },
        AgentModel {
            id: "minimax-m2.7".into(),
            name: "MiniMax-M2.7".into(),
            protocol: Protocol::OpenAiChat,
            url: CHAT_URL.into(),
            reasoning: true,
            context_window: 204_800,
            max_tokens: 131_072,
        },
        AgentModel {
            id: "qwen3.6-plus".into(),
            name: "Qwen3.6 Plus".into(),
            protocol: Protocol::OpenAiChat,
            url: CHAT_URL.into(),
            reasoning: true,
            context_window: 1_000_000,
            max_tokens: 65_536,
        },
    ]
}

fn openai_responses_models() -> Vec<AgentModel> {
    vec![
        AgentModel {
            id: "gpt-5.6-luna".into(),
            name: "GPT-5.6 Luna (2x usage)".into(),
            protocol: Protocol::OpenAiResponses,
            url: RESPONSES_URL.into(),
            reasoning: true,
            context_window: 1_050_000,
            max_tokens: 128_000,
        },
        AgentModel {
            id: "grok-4.5".into(),
            name: "Grok 4.5".into(),
            protocol: Protocol::OpenAiResponses,
            url: RESPONSES_URL.into(),
            reasoning: true,
            context_window: 500_000,
            max_tokens: 498_976,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baked_presets_contain_opencode_with_expected_models() {
        // 直接验证 baked 基线，避免受本地 `~/.pi/agent/models-store.json` 动态覆盖影响
        let models = baked_models();
        assert_eq!(models.len(), 18);
        assert!(
            models
                .iter()
                .any(|m| m.id == "minimax-m3" && m.protocol == Protocol::AnthropicMessages)
        );
        assert!(
            models
                .iter()
                .any(|m| m.id == "deepseek-v4-flash" && m.protocol == Protocol::OpenAiChat)
        );
        assert!(
            models
                .iter()
                .any(|m| m.id == "gpt-5.6-luna" && m.protocol == Protocol::OpenAiResponses)
        );
        // URL 正确性
        assert!(
            models
                .iter()
                .filter(|m| m.protocol == Protocol::AnthropicMessages)
                .all(|m| m.url == ANTHROPIC_URL)
        );
        assert!(
            models
                .iter()
                .filter(|m| m.protocol == Protocol::OpenAiChat)
                .all(|m| m.url == CHAT_URL)
        );
    }

    #[test]
    fn builtin_presets_contain_opencode_with_expected_models() {
        // 外部 pi 兼容供应商（~/.config/crossh/agent/providers/*.json）会追加到内置列表，
        // 测试仅校验 opencode 存在且模型完整，不校验总数。
        let presets = builtin_presets();
        assert!(presets.iter().any(|p| p.id == OPENCODE_ID));
        let go = presets.iter().find(|p| p.id == OPENCODE_ID).unwrap();
        assert_eq!(go.id, "opencode");
        // 动态 overlay 可能使数量大于 baked 基线（pi 侧 4h 刷新追加模型）
        assert!(go.models.len() >= 18);
        assert!(go.models.iter().any(|m| m.id == "minimax-m3"));
        // 校验 URL 已修正为不含 /go
        assert!(go.models.iter().any(|m| m.url == ANTHROPIC_URL));
        assert!(go.models.iter().any(|m| m.url == CHAT_URL));
        assert!(go.models.iter().any(|m| m.url == RESPONSES_URL));
    }
    #[test]
    fn merge_models_overlays_dynamic_onto_baseline() {
        let baseline = vec![
            AgentModel {
                id: "a".into(),
                name: "A".into(),
                protocol: Protocol::OpenAiChat,
                url: CHAT_URL.into(),
                reasoning: false,
                context_window: 100,
                max_tokens: 10,
            },
            AgentModel {
                id: "b".into(),
                name: "B".into(),
                protocol: Protocol::OpenAiChat,
                url: CHAT_URL.into(),
                reasoning: false,
                context_window: 100,
                max_tokens: 10,
            },
        ];
        let dynamic = vec![
            AgentModel {
                id: "b".into(),
                name: "B-new".into(),
                protocol: Protocol::AnthropicMessages,
                url: ANTHROPIC_URL.into(),
                reasoning: true,
                context_window: 200,
                max_tokens: 20,
            },
            AgentModel {
                id: "c".into(),
                name: "C".into(),
                protocol: Protocol::OpenAiChat,
                url: CHAT_URL.into(),
                reasoning: true,
                context_window: 300,
                max_tokens: 30,
            },
        ];
        let merged = merge_models(baseline, dynamic);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[1].name, "B-new");
        assert!(merged.iter().any(|m| m.id == "c"));
    }
}
