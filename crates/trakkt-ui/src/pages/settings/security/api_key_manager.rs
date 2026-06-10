// SPDX-License-Identifier: AGPL-3.0-or-later

//! API Key Management card — security settings section for REST/MCP API keys.
//!
//! Shows:
//! - List of API keys with name, prefix, scopes, created date, last used
//! - Create button opening a modal with name, scope checkboxes, expiry
//! - One-time token display after creation with copy button
//! - Revoke button on each active key with confirm dialog
//! - Empty state when no keys exist

use leptos::prelude::*;
use phosphor_leptos::Icon;
use crate::components::{
    Alert, AlertDescription, AlertVariant, Badge, BadgeVariant, Button, ButtonSize, ButtonVariant,
    Card, CardContent, CardDescription, CardHeader, CardTitle, ConfirmDialog, EmptyState,
    Checkbox, Modal, ModalSize, Select, SelectVariant,
};
use crate::components::INPUT_CLASS;
use crate::server_fns::security::{
    list_api_keys, create_api_key, revoke_api_key, ApiKeyEntry, CreateApiKeyResult,
};
use crate::utils::date::format_date_locale;

/// Available scopes for API keys.
const AVAILABLE_SCOPES: &[(&str, &str)] = &[
    ("issues:read", "Read issues"),
    ("issues:write", "Create/update issues"),
    ("comments:write", "Add comments"),
    ("attachments:read", "Download/list attachments"),
    ("attachments:write", "Manage issue attachments"),
    ("labels:read", "Read labels"),
    ("labels:write", "Create/update labels"),
    ("teams:read", "Read teams"),
    ("teams:write", "Update team settings"),
    ("projects:read", "Read projects"),
    ("projects:write", "Create/update projects"),
];

/// Copy text to the clipboard and return success (client-side only).
#[cfg(target_arch = "wasm32")]
async fn copy_to_clipboard(text: &str) -> bool {
    if let Some(window) = web_sys::window() {
        let clipboard = window.navigator().clipboard();
        let promise = clipboard.write_text(text);
        wasm_bindgen_futures::JsFuture::from(promise).await.is_ok()
    } else {
        false
    }
}


/// API Key Management component.
#[component]
pub fn ApiKeyManager() -> impl IntoView {
    // ── Server data ──────────────────────────────────────────────────────
    let keys_resource = Resource::new(|| (), |_| list_api_keys());

    // ── UI state ─────────────────────────────────────────────────────────
    let loading = RwSignal::new(true);
    let keys = RwSignal::new(Vec::<ApiKeyEntry>::new());
    let error = RwSignal::new(Option::<String>::None);

    // ── Create modal state ───────────────────────────────────────────────
    let create_modal_open = RwSignal::new(false);
    let create_name = RwSignal::new(String::new());
    let create_scopes = RwSignal::new(Vec::<String>::new());
    let create_expires = RwSignal::new(Option::<i32>::None);
    let (expiry_str, set_expiry_str) = signal(String::new());
    let creating = RwSignal::new(false);
    let created_token = RwSignal::new(Option::<CreateApiKeyResult>::None);
    let copied = RwSignal::new(false);
    let copied_curl = RwSignal::new(false);
    let copied_mcp = RwSignal::new(false);

    // ── Confirm dialog state ─────────────────────────────────────────────
    let dialog_open = RwSignal::new(false);
    let pending_revoke_id = RwSignal::new(Option::<String>::None);
    let pending_revoke_name = RwSignal::new(String::new());

    // ── Sync resource into signals ───────────────────────────────────────
    Effect::new(move || {
        if let Some(result) = keys_resource.get() {
            loading.set(false);
            match result {
                Ok(data) => {
                    error.set(None);
                    keys.set(data);
                }
                Err(e) => {
                    error.set(Some(format!("Failed to load API keys: {e}")));
                }
            }
        }
    });

    // ── Create flow handlers ─────────────────────────────────────────────
    let open_create_modal = move |_| {
        create_name.set(String::new());
        create_scopes.set(Vec::new());
        create_expires.set(None);
        set_expiry_str.set(String::new());
        created_token.set(None);
        copied.set(false);
        copied_curl.set(false);
        copied_mcp.set(false);
        create_modal_open.set(true);
    };

    let close_create_modal = Callback::new(move |()| {
        create_modal_open.set(false);
        // If a token was just created, refresh the list
        if created_token.get_untracked().is_some() {
            loading.set(true);
            error.set(None);
            leptos::task::spawn_local(async move {
                match list_api_keys().await {
                    Ok(data) => {
                        keys.set(data);
                        loading.set(false);
                    }
                    Err(e) => {
                        error.set(Some(format!("Failed to reload API keys: {e}")));
                        loading.set(false);
                    }
                }
            });
        }
    });

    let handle_create = move |_| {
        let name = create_name.get_untracked();
        let scopes = create_scopes.get_untracked().join(",");
        let expires = create_expires.get_untracked();

        creating.set(true);
        error.set(None);

        leptos::task::spawn_local(async move {
            match create_api_key(name, scopes, expires).await {
                Ok(result) => {
                    created_token.set(Some(result));
                    creating.set(false);
                }
                Err(e) => {
                    error.set(Some(format!("Failed to create API key: {e}")));
                    creating.set(false);
                }
            }
        });
    };

    // Copy `text` to the clipboard and flash the given `flag` signal for 2 s.
    let copy_text = move |text: String, flag: RwSignal<bool>| {
        leptos::task::spawn_local(async move {
            #[cfg(target_arch = "wasm32")]
            {
                if copy_to_clipboard(&text).await {
                    flag.set(true);
                    gloo_timers::future::TimeoutFuture::new(2000).await;
                    flag.set(false);
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let _ = (text, flag);
            }
        });
    };

    let handle_copy = move |_| {
        if let Some(token_result) = created_token.get_untracked() {
            copy_text(token_result.token, copied);
        }
    };

    // ── Scope toggle helper ──────────────────────────────────────────────
    let toggle_scope = move |scope: String, checked: bool| {
        create_scopes.update(|scopes| {
            if checked {
                if !scopes.contains(&scope) {
                    scopes.push(scope);
                }
            } else {
                scopes.retain(|s| s != &scope);
            }
        });
    };

    // ── Revoke flow handlers ─────────────────────────────────────────────
    let open_revoke_dialog = move |key: ApiKeyEntry| {
        pending_revoke_id.set(Some(key.token_id.clone()));
        pending_revoke_name.set(key.name.clone());
        dialog_open.set(true);
    };

    let on_confirm_revoke = Callback::new(move |()| {
        dialog_open.set(false);
        let Some(token_id) = pending_revoke_id.get_untracked() else {
            return;
        };
        pending_revoke_id.set(None);
        error.set(None);

        leptos::task::spawn_local(async move {
            match revoke_api_key(token_id).await {
                Ok(_) => {
                    loading.set(true);
                    match list_api_keys().await {
                        Ok(data) => {
                            keys.set(data);
                            loading.set(false);
                        }
                        Err(e) => {
                            error.set(Some(format!("Failed to reload API keys: {e}")));
                            loading.set(false);
                        }
                    }
                }
                Err(e) => {
                    error.set(Some(format!("Failed to revoke API key: {e}")));
                }
            }
        });
    });

    let on_cancel_revoke = Callback::new(move |()| {
        dialog_open.set(false);
        pending_revoke_id.set(None);
    });

    view! {
        <Card>
            <CardHeader>
                <div class="flex items-center justify-between">
                    <div>
                        <CardTitle>"API Keys"</CardTitle>
                        <CardDescription>"Create and manage API keys for REST API and MCP access."</CardDescription>
                    </div>
                    <Button
                        variant=ButtonVariant::Default
                        on:click=open_create_modal
                    >
                        <Icon icon=phosphor_leptos::PLUS size="16px"/>
                        "Create API Key"
                    </Button>
                </div>
            </CardHeader>
            <CardContent>
                // Error alert
                <Show when=move || error.get().is_some()>
                    <div class="mb-6">
                        <Alert variant=AlertVariant::Error>
                            <AlertDescription>
                                {move || error.get().unwrap_or_default()}
                            </AlertDescription>
                        </Alert>
                    </div>
                </Show>

                <div class="mb-6">
                    // Loading state
                    <Show
                        when=move || !(loading.get() && keys.get().is_empty())
                        fallback=|| view! {
                            <div class="text-center py-8">
                                <span class="animate-spin h-8 w-8 border-2 border-primary border-t-transparent rounded-full inline-block"/>
                                <p class="text-muted-foreground mt-2">"Loading API keys..."</p>
                            </div>
                        }
                    >
                        // Empty state
                        <Show
                            when=move || !keys.get().is_empty()
                            fallback=|| view! {
                                <EmptyState
                                    title="No API keys"
                                    description="Create an API key to access the REST API or connect MCP clients."
                                    class="border-2 border-dashed bg-muted"
                                />
                            }
                        >
                            // Keys table
                            <div class="overflow-x-auto">
                                <table class="min-w-full divide-y divide-border">
                                    <thead class="bg-muted">
                                        <tr>
                                            <th class="px-6 py-3 text-left text-xs font-medium text-muted-foreground uppercase tracking-wider">
                                                "Name"
                                            </th>
                                            <th class="px-6 py-3 text-left text-xs font-medium text-muted-foreground uppercase tracking-wider">
                                                "Key"
                                            </th>
                                            <th class="px-6 py-3 text-left text-xs font-medium text-muted-foreground uppercase tracking-wider">
                                                "Scopes"
                                            </th>
                                            <th class="px-6 py-3 text-left text-xs font-medium text-muted-foreground uppercase tracking-wider">
                                                "Created"
                                            </th>
                                            <th class="px-6 py-3 text-left text-xs font-medium text-muted-foreground uppercase tracking-wider">
                                                "Last Used"
                                            </th>
                                            <th class="px-6 py-3 text-right text-xs font-medium text-muted-foreground uppercase tracking-wider">
                                                "Actions"
                                            </th>
                                        </tr>
                                    </thead>
                                    <tbody class="bg-background divide-y divide-border">
                                        <For
                                            each=move || keys.get()
                                            key=|k| k.token_id.clone()
                                            let:key
                                        >
                                            <ApiKeyRow
                                                key=key
                                                on_revoke=Callback::new(open_revoke_dialog)
                                            />
                                        </For>
                                    </tbody>
                                </table>
                            </div>
                        </Show>
                    </Show>
                </div>

                // Info alert
                <Alert variant=AlertVariant::Info>
                    <AlertDescription>
                        <strong>"Tip:"</strong>
                        " API keys provide access to the REST API. Use them for CI/CD integrations, scripts, or MCP client connections."
                    </AlertDescription>
                </Alert>
            </CardContent>
        </Card>

        // Confirm Revoke Dialog
        {move || {
            let name = pending_revoke_name.get();
            view! {
                <ConfirmDialog
                    open=Signal::from(dialog_open)
                    title="Revoke API Key?"
                    message=format!("Are you sure you want to revoke \"{name}\"? This action cannot be undone and any integrations using this key will stop working.")
                    confirm_text="Revoke"
                    destructive=true
                    on_confirm=on_confirm_revoke
                    on_cancel=on_cancel_revoke
                />
            }
        }}

        // Create API Key Modal
        <Modal
            show=Signal::from(create_modal_open)
            on_close=close_create_modal
            title="Create API Key"
            size=ModalSize::Lg
        >
            <Show
                when=move || created_token.get().is_none()
                fallback=move || {
                    // Success state — show the token + usage instructions
                    let token_display = move || {
                        created_token.get().map(|r| r.token).unwrap_or_default()
                    };
                    #[cfg(target_arch = "wasm32")]
                    let origin = web_sys::window()
                        .and_then(|w| w.location().origin().ok())
                        .unwrap_or_else(|| "https://trakkt.app".to_string());
                    #[cfg(not(target_arch = "wasm32"))]
                    let origin = "https://trakkt.app".to_string();
                    let origin_a = origin.clone();
                    let origin_b = origin.clone();
                    let origin_c = origin.clone();
                    let origin_d = origin;
                    let curl_snippet = move || {
                        let tok = created_token.get().map(|r| r.token).unwrap_or_default();
                        format!("curl -H \"Authorization: Bearer {tok}\" {origin_a}/api/v1/issues")
                    };
                    let curl_snippet_copy = move || {
                        let tok = created_token.get().map(|r| r.token).unwrap_or_default();
                        format!("curl -H \"Authorization: Bearer {tok}\" {origin_b}/api/v1/issues")
                    };
                    let mcp_snippet = move || {
                        let tok = created_token.get().map(|r| r.token).unwrap_or_default();
                        format!(
                            "{{\n  \"mcpServers\": {{\n    \"trakkt\": {{\n      \"command\": \"npx\",\n      \"args\": [\"-y\", \"@anthropic-ai/mcp-remote\", \"{origin_c}/mcp\"],\n      \"env\": {{\n        \"TRAKKT_API_KEY\": \"{tok}\"\n      }}\n    }}\n  }}\n}}"
                        )
                    };
                    let mcp_snippet_copy = move || {
                        let tok = created_token.get().map(|r| r.token).unwrap_or_default();
                        format!(
                            "{{\n  \"mcpServers\": {{\n    \"trakkt\": {{\n      \"command\": \"npx\",\n      \"args\": [\"-y\", \"@anthropic-ai/mcp-remote\", \"{origin_d}/mcp\"],\n      \"env\": {{\n        \"TRAKKT_API_KEY\": \"{tok}\"\n      }}\n    }}\n  }}\n}}"
                        )
                    };
                    let scopes_snapshot = create_scopes.get_untracked();

                    view! {
                        <div class="space-y-5">
                            // Warning — always at the top
                            <Alert variant=AlertVariant::Warning>
                                <AlertDescription>
                                    <strong>"Important:"</strong>
                                    " Make sure to copy your API key now. You won\u{2019}t be able to see it again!"
                                </AlertDescription>
                            </Alert>

                            // ── Token ────────────────────────────────────────
                            <div class="space-y-2">
                                <label class="text-sm font-medium text-foreground">"Your API Key"</label>
                                <div class="flex items-center gap-2">
                                    <code class="flex-1 px-3 py-2 bg-muted border border-border rounded-md text-sm font-mono break-all select-all">
                                        {token_display}
                                    </code>
                                    <Button
                                        variant=ButtonVariant::Secondary
                                        size=ButtonSize::Sm
                                        on:click=handle_copy
                                    >
                                        {move || if copied.get() { "Copied!" } else { "Copy" }}
                                    </Button>
                                </div>
                            </div>

                            // ── Usage instructions ───────────────────────────
                            <div class="space-y-4 border-t border-border pt-4">
                                <h3 class="text-sm font-medium text-foreground">"Usage"</h3>

                                // curl example
                                <div class="space-y-1.5">
                                    <div class="flex items-center justify-between">
                                        <label class="text-xs font-medium text-muted-foreground">"REST API (curl)"</label>
                                        <Button
                                            variant=ButtonVariant::Ghost
                                            size=ButtonSize::Sm
                                            on:click=move |_| copy_text(curl_snippet_copy(), copied_curl)
                                        >
                                            {move || if copied_curl.get() { "Copied!" } else { "Copy" }}
                                        </Button>
                                    </div>
                                    <pre class="px-3 py-2 bg-muted border border-border rounded-md text-xs font-mono whitespace-pre-wrap break-all select-all overflow-x-auto">
                                        {curl_snippet}
                                    </pre>
                                </div>

                                // MCP config
                                <div class="space-y-1.5">
                                    <div class="flex items-center justify-between">
                                        <label class="text-xs font-medium text-muted-foreground">"MCP Setup (Claude Code)"</label>
                                        <Button
                                            variant=ButtonVariant::Ghost
                                            size=ButtonSize::Sm
                                            on:click=move |_| copy_text(mcp_snippet_copy(), copied_mcp)
                                        >
                                            {move || if copied_mcp.get() { "Copied!" } else { "Copy" }}
                                        </Button>
                                    </div>
                                    <pre class="px-3 py-2 bg-muted border border-border rounded-md text-xs font-mono whitespace-pre-wrap break-all select-all overflow-x-auto">
                                        {mcp_snippet}
                                    </pre>
                                </div>
                            </div>

                            // ── Scope summary ────────────────────────────────
                            <div class="space-y-2 border-t border-border pt-4">
                                <h3 class="text-sm font-medium text-foreground">"Scopes"</h3>
                                {if scopes_snapshot.is_empty() {
                                    view! {
                                        <p class="text-xs text-muted-foreground">"No scopes selected \u{2014} this key has no permissions."</p>
                                    }.into_any()
                                } else {
                                    view! {
                                        <div class="flex flex-wrap gap-1.5">
                                            {scopes_snapshot
                                                .into_iter()
                                                .map(|scope| view! {
                                                    <Badge variant=BadgeVariant::Secondary>
                                                        <span class="text-xs font-mono">{scope}</span>
                                                    </Badge>
                                                })
                                                .collect_view()}
                                        </div>
                                    }.into_any()
                                }}
                            </div>

                            // ── Done button ──────────────────────────────────
                            <div class="flex justify-end pt-2">
                                <Button
                                    variant=ButtonVariant::Default
                                    on:click=move |_| close_create_modal.run(())
                                >
                                    "Done"
                                </Button>
                            </div>
                        </div>
                    }
                }
            >
                // Create form
                <div class="space-y-5">
                    // Name field
                    <div class="space-y-2">
                        <label class="text-sm font-medium text-foreground">"Name"</label>
                        <input
                            type="text"
                            class=INPUT_CLASS
                            placeholder="e.g. CI/CD Pipeline, MCP Client"
                            prop:value=move || create_name.get()
                            on:input=move |ev| {
                                create_name.set(event_target_value(&ev));
                            }
                        />
                    </div>

                    // Scopes
                    <div class="space-y-2">
                        <label class="text-sm font-medium text-foreground">"Permissions"</label>
                        <p class="text-xs text-muted-foreground">"Select the scopes this key should have access to."</p>
                        <div class="grid grid-cols-1 gap-2 pt-1">
                            {AVAILABLE_SCOPES
                                .iter()
                                .map(|(scope, label)| {
                                    let scope_str = scope.to_string();
                                    let scope_for_check = scope_str.clone();
                                    let scope_for_toggle = scope_str.clone();
                                    let is_checked = Signal::derive(move || {
                                        create_scopes.get().contains(&scope_for_check)
                                    });
                                    view! {
                                        <label class="flex items-center gap-2.5 py-1 cursor-pointer">
                                            <Checkbox
                                                checked=is_checked
                                                on_change=Callback::new(move |checked: bool| {
                                                    toggle_scope(scope_for_toggle.clone(), checked);
                                                })
                                            />
                                            <span class="text-sm text-foreground">{*label}</span>
                                            <span class="text-xs text-muted-foreground font-mono">{*scope}</span>
                                        </label>
                                    }
                                })
                                .collect_view()}
                        </div>
                    </div>

                    // Expiry
                    <div class="space-y-2">
                        <label class="text-sm font-medium text-foreground">"Expiration"</label>
                        <Select
                            value=expiry_str
                            options=Signal::derive(|| vec![
                                ("".to_string(), "No expiration".to_string()),
                                ("30".to_string(), "30 days".to_string()),
                                ("60".to_string(), "60 days".to_string()),
                                ("90".to_string(), "90 days".to_string()),
                                ("365".to_string(), "1 year".to_string()),
                            ])
                            on_change=Callback::new(move |val: String| {
                                set_expiry_str.set(val.clone());
                                create_expires.set(match val.as_str() {
                                    "30" => Some(30),
                                    "60" => Some(60),
                                    "90" => Some(90),
                                    "365" => Some(365),
                                    _ => None,
                                });
                            })
                            variant=SelectVariant::Form
                            placeholder="No expiration"
                        />
                    </div>

                    // Actions
                    <div class="flex justify-end gap-2 pt-2">
                        <Button
                            variant=ButtonVariant::Ghost
                            on:click=move |_| close_create_modal.run(())
                        >
                            "Cancel"
                        </Button>
                        <Button
                            variant=ButtonVariant::Default
                            on:click=handle_create
                            disabled=Signal::derive(move || creating.get() || create_name.get().trim().is_empty())
                        >
                            {move || if creating.get() { "Creating..." } else { "Create Key" }}
                        </Button>
                    </div>
                </div>
            </Show>
        </Modal>
    }
}

/// A single row in the API keys table.
#[component]
fn ApiKeyRow(
    key: ApiKeyEntry,
    on_revoke: Callback<ApiKeyEntry>,
) -> impl IntoView {
    let is_active = key.active;
    let prefix_display = key
        .token_prefix
        .clone()
        .map(|p| format!("{p}..."))
        .unwrap_or_else(|| "\u{2014}".to_string());

    let scopes_display = if key.scopes.is_empty() {
        vec!["Full access".to_string()]
    } else {
        key.scopes.clone()
    };

    let created = format_date_locale(&key.created_at);
    let last_used = key
        .last_used
        .as_deref()
        .map(format_date_locale)
        .unwrap_or_else(|| "Never".to_string());

    let key_for_revoke = key.clone();

    view! {
        <tr class=if !is_active { "opacity-60" } else { "" }>
            // Name column
            <td class="px-6 py-4 whitespace-nowrap">
                <div class="flex items-center gap-2">
                    <span class="text-sm font-medium text-foreground">{key.name.clone()}</span>
                    {(!is_active).then(|| view! {
                        <Badge variant=BadgeVariant::Secondary>"Revoked"</Badge>
                    })}
                </div>
            </td>

            // Prefix column
            <td class="px-6 py-4 whitespace-nowrap">
                <code class="text-xs text-muted-foreground font-mono">{prefix_display}</code>
            </td>

            // Scopes column
            <td class="px-6 py-4">
                <div class="flex flex-wrap gap-1">
                    {scopes_display
                        .into_iter()
                        .map(|scope| view! {
                            <Badge variant=BadgeVariant::Secondary>
                                <span class="text-xs">{scope}</span>
                            </Badge>
                        })
                        .collect_view()}
                </div>
            </td>

            // Created column
            <td class="px-6 py-4 whitespace-nowrap text-sm text-muted-foreground">
                {created}
            </td>

            // Last Used column
            <td class="px-6 py-4 whitespace-nowrap text-sm text-muted-foreground">
                {last_used}
            </td>

            // Actions column
            <td class="px-6 py-4 whitespace-nowrap text-right">
                {is_active.then(|| {
                    let key_clone = key_for_revoke.clone();
                    view! {
                        <Button
                            variant=ButtonVariant::Ghost
                            size=ButtonSize::Sm
                            on:click=move |_| on_revoke.run(key_clone.clone())
                        >
                            "Revoke"
                        </Button>
                    }
                })}
            </td>
        </tr>
    }
}
