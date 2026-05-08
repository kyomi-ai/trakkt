// SPDX-License-Identifier: AGPL-3.0-or-later

//! User avatar component with initials fallback.
//!
//! DESIGN.md: "Assignee: avatar circle, w-[18px] h-[18px] rounded-full"
//!
//! Sizes:
//! - Sm (18px): issue rows, compact lists
//! - Md (28px): comments, activity feed
//! - Lg (36px): profile, user detail

use leptos::prelude::*;

/// Avatar display size.
#[derive(Clone, Copy, Default, PartialEq)]
pub enum AvatarSize {
    /// 18px — issue rows, compact lists.
    #[default]
    Sm,
    /// 28px — comments, activity feed.
    Md,
    /// 36px — profile, user detail.
    Lg,
}

impl AvatarSize {
    fn class(self) -> &'static str {
        match self {
            Self::Sm => "w-[18px] h-[18px] text-[8px]",
            Self::Md => "w-7 h-7 text-[11px]",
            Self::Lg => "w-9 h-9 text-sm",
        }
    }
}

/// Extract up to two initials from a name string.
fn extract_initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase()
}

/// User avatar showing initials (or a future image).
///
/// # Usage
/// ```ignore
/// <Avatar name="Jason Park".to_string() size=AvatarSize::Sm/>
/// ```
#[component]
pub fn Avatar(
    /// User display name — used for initials fallback.
    #[prop(into, optional)]
    name: Option<String>,
    /// Avatar image URL (not yet used — renders initials for now).
    #[prop(into, optional)]
    image_url: Option<String>,
    /// Size variant. Default: Sm (18px).
    #[prop(default = AvatarSize::Sm)]
    size: AvatarSize,
) -> impl IntoView {
    let initials = extract_initials(name.as_deref().unwrap_or("?"));

    // When image URLs are available, this will render an <img> with initials fallback.
    let _image_url = image_url;

    view! {
        <span
            class={format!(
                "inline-flex items-center justify-center rounded-full bg-muted text-muted-foreground font-medium shrink-0 {}",
                size.class()
            )}
            title=name
        >
            {initials}
        </span>
    }
}
