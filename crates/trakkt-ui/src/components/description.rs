// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared issue/project Markdown rendering and editing through kode.

use leptos::prelude::*;
use std::sync::Arc;

/// Build a kode `Theme` matching Trakkt's design system (warm light palette).
///
/// Since kode's `Theme` is `#[non_exhaustive]`, we start from `Theme::light()`
/// and override the fields we need.
pub(crate) fn trakkt_kode_theme() -> kode_leptos::Theme {
    let mut t = kode_leptos::Theme::light();
    // Colors use CSS var() references so they follow Trakkt's light/dark
    // mode automatically. The actual values live in main.css :root block
    // which maps --kode-* vars to --color-* design tokens.
    t.bg = "var(--color-card)";
    t.fg = "var(--color-foreground)";
    t.fg_bright = "var(--color-foreground)";
    t.fg_dim = "var(--color-muted-foreground)";
    t.cursor = "var(--color-foreground)";
    t.selection = "rgba(13, 148, 136, 0.15)";
    t.current_line = "transparent";
    t.gutter_fg = "var(--color-muted-foreground)";
    t.gutter_border = "var(--color-border)";
    t.border = "var(--color-border)";
    t.accent = "var(--color-primary)";
    t.bg_highlight = "var(--color-accent)";
    t.bg_hover = "var(--color-accent)";
    t.marker_error = "#DC2626";
    t.marker_warning = "#CA8A04";
    t.marker_info = "#2563EB";
    t.marker_hint = "var(--color-muted-foreground)";
    t.code_fg = "var(--color-primary)";
    t.link = "var(--color-primary)";
    t.syntax = kode_leptos::SyntaxTheme::GithubLight;
    // Typography — DESIGN.md fonts
    t.content_font_family = Some("'DM Sans', sans-serif");
    t.heading_font_family = Some("'Instrument Serif', serif");
    t.code_font_family = Some("'Geist Mono', monospace");
    t.font_family = Some("'Geist Mono', monospace");
    // Content layout
    t.content_max_width = Some("100%");
    t.container_padding = Some("0");
    // Toolbar styling — also uses CSS vars for dark mode
    t.toolbar_bg = Some("var(--color-card)");
    t.toolbar_border_color = Some("var(--color-border)");
    t.toolbar_button_border_radius = Some("6px");
    t.toolbar_button_hover_bg = Some("var(--color-accent)");
    t.toolbar_button_selected_bg = Some("var(--color-primary)");
    t.toolbar_button_selected_color = Some("#FFFFFF");
    // Heading styling
    t.heading_font_weight = Some("600");
    t.h1_border_width = Some("0");
    t.h2_border_width = Some("0");
    t
}

/// The same renderer, safe-content handling and theme for issue and project
/// descriptions. Persistence and draft ownership belong to the caller.
#[component]
pub fn MarkdownDescription(
    #[prop(into)] content: Signal<String>,
    #[prop(optional)] on_change: Option<Arc<dyn Fn(String) + Send + Sync>>,
    #[prop(default = false)] readonly: bool,
    #[prop(default = false)] autofocus: bool,
    #[prop(optional)] on_upload: Option<Arc<dyn Fn(kode_leptos::UploadTrigger) + Send + Sync>>,
    #[prop(optional)] on_delete_attachment: Option<
        Arc<dyn Fn(kode_leptos::DeleteAttachmentRequest) + Send + Sync>,
    >,
    #[prop(optional)] on_click_attachment: Option<
        Arc<dyn Fn(kode_leptos::ClickAttachmentRequest) + Send + Sync>,
    >,
    #[prop(optional)] upload_complete: Option<RwSignal<Option<kode_leptos::UploadComplete>>>,
    #[prop(default = vec![])] extensions: Vec<Arc<dyn kode_leptos::Extension>>,
) -> impl IntoView {
    let theme_state = use_context::<crate::components::theme::ThemeState>();
    let theme = Signal::derive(move || {
        let mut theme = trakkt_kode_theme();
        theme.content_padding = Some("0");
        theme.bg = "var(--color-background)";
        if let Some(state) = theme_state
            && state.effective.get() == "dark"
        {
            theme.syntax = kode_leptos::SyntaxTheme::OneDark;
        }
        theme
    });

    let container = NodeRef::<leptos::html::Div>::new();
    if autofocus && !readonly {
        Effect::new(move || {
            let _container = container.get();
            #[cfg(target_arch = "wasm32")]
            if let Some(container) = _container {
                use wasm_bindgen::JsCast;
                if let Ok(Some(editor)) = container.query_selector("[contenteditable='true']")
                    && let Some(editor) = editor.dyn_ref::<web_sys::HtmlElement>()
                    && let Err(error) = editor.focus()
                {
                    tracing::warn!("Could not focus description editor: {error:?}");
                }
            }
        });
    }

    view! {
        <style>{include_str!("description.css")}</style>
        <div class="markdown-description min-w-0 max-w-full" node_ref=container>
            <kode_leptos::TreeWysiwygEditor
                content=content
                nostrip:on_change=on_change
                readonly=readonly
                show_fixed_toolbar=false
                show_floating_toolbar=true
                theme=theme
                nostrip:on_upload=on_upload
                nostrip:on_delete_attachment=on_delete_attachment
                nostrip:on_click_attachment=on_click_attachment
                nostrip:upload_complete=upload_complete
                extensions=extensions
            />
        </div>
    }
}
