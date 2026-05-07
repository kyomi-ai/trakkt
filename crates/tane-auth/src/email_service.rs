// SPDX-License-Identifier: AGPL-3.0-or-later

//! SMTP email service for sending transactional emails.
//!
//! Wire-compatible with Python's `services/email_service.py`.
//!
//! Configuration via environment variables:
//! - `SMTP_HOST`, `SMTP_PORT`, `SMTP_USER`, `SMTP_PASSWORD`
//! - `SMTP_FROM_EMAIL` (default: `noreply@tane.ai`)
//! - `SMTP_FROM_NAME` (default: `Tane`)
//!
//! Graceful degradation: if SMTP is not configured, `send_email` logs a warning
//! and returns `false` — it never fails the calling operation.

use lettre::{
    message::{header::ContentType, Attachment, Body, Mailbox, MultiPart, SinglePart},
    transport::smtp::authentication::Credentials,
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};

/// Tane logo PNG, embedded at compile time.
static LOGO_BYTES: &[u8] =
    include_bytes!("../../../assets/tane_email_logo.png");

/// Content-ID used in `<img src="cid:tane_logo">`.
const LOGO_CID: &str = "tane_logo";

/// SMTP email service.
///
/// Constructed once and shared (e.g., via `Arc` or created on-demand per call).
/// All methods are `&self` — the struct is cheaply cloneable when wrapped in Arc.
#[derive(Debug, Clone)]
pub struct EmailService {
    smtp_host: Option<String>,
    smtp_port: u16,
    smtp_user: Option<String>,
    smtp_password: Option<String>,
    from_email: String,
    from_name: String,
    /// Base URL for the frontend app (e.g. `https://app.tane.ai`).
    frontend_url: String,
}

impl EmailService {
    /// Create a new `EmailService` reading configuration from environment variables.
    pub fn from_env() -> Self {
        let smtp_host = std::env::var("SMTP_HOST").ok();
        let smtp_port: u16 = std::env::var("SMTP_PORT")
            .unwrap_or_else(|_| "587".to_string())
            .parse()
            .unwrap_or(587);
        let smtp_user = std::env::var("SMTP_USER").ok();
        let smtp_password = std::env::var("SMTP_PASSWORD").ok();
        let from_email =
            std::env::var("SMTP_FROM_EMAIL").unwrap_or_else(|_| "noreply@tane.ai".to_string());
        let from_name =
            std::env::var("SMTP_FROM_NAME").unwrap_or_else(|_| "Tane".to_string());
        let frontend_url = std::env::var("FRONTEND_URL")
            .unwrap_or_else(|_| "https://app.tane.ai".to_string())
            .trim_end_matches('/')
            .to_string();

        let configured = smtp_host.is_some() && smtp_user.is_some() && smtp_password.is_some();
        if !configured {
            tracing::warn!(
                "SMTP not configured. Email sending will be disabled. \
                 Set SMTP_HOST, SMTP_PORT, SMTP_USER, SMTP_PASSWORD in .env"
            );
        }

        Self {
            smtp_host,
            smtp_port,
            smtp_user,
            smtp_password,
            from_email,
            from_name,
            frontend_url,
        }
    }

    /// Check if SMTP is configured (all required env vars are set).
    pub fn is_configured(&self) -> bool {
        self.smtp_host.is_some() && self.smtp_user.is_some() && self.smtp_password.is_some()
    }

    /// Send an email via SMTP.
    ///
    /// Returns `true` if the email was sent successfully, `false` otherwise.
    /// Never panics or returns an error — logs warnings on failure.
    ///
    /// `reply_to` sets the Reply-To header so recipients can reply directly
    /// to the relevant person (e.g., the user who submitted feedback).
    ///
    /// `images` is an optional list of `(content_id, png_bytes)` pairs for
    /// additional inline CID images (e.g. rendered charts). Pass `&[]` when
    /// no extra images are needed.
    pub async fn send_email(
        &self,
        to_email: &str,
        subject: &str,
        html_body: &str,
        text_body: Option<&str>,
        reply_to: Option<&str>,
        images: &[(String, Vec<u8>)],
    ) -> bool {
        if !self.is_configured() {
            tracing::warn!(
                to = %to_email,
                "SMTP not configured. Skipping email."
            );
            return false;
        }

        let (Some(smtp_host), Some(smtp_user), Some(smtp_password)) = (
            self.smtp_host.as_deref(),
            self.smtp_user.as_deref(),
            self.smtp_password.as_deref(),
        ) else {
            tracing::error!("SMTP config missing despite is_configured() check");
            return false;
        };

        // Build the From mailbox
        let from_mailbox: Mailbox = match format!("{} <{}>", self.from_name, self.from_email).parse()
        {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(
                    "Failed to parse From address '{}': {}",
                    self.from_email,
                    e
                );
                return false;
            }
        };

        // Build the To mailbox
        let to_mailbox: Mailbox = match to_email.parse() {
            Ok(m) => m,
            Err(e) => {
                tracing::error!("Failed to parse To address '{}': {}", to_email, e);
                return false;
            }
        };

        // Build multipart/alternative message (text + html)
        let alternative = if let Some(text) = text_body {
            MultiPart::alternative()
                .singlepart(
                    SinglePart::builder()
                        .header(ContentType::TEXT_PLAIN)
                        .body(text.to_string()),
                )
                .singlepart(
                    SinglePart::builder()
                        .header(ContentType::TEXT_HTML)
                        .body(html_body.to_string()),
                )
        } else {
            MultiPart::alternative().singlepart(
                SinglePart::builder()
                    .header(ContentType::TEXT_HTML)
                    .body(html_body.to_string()),
            )
        };

        // Wrap in multipart/related so the inline logo CID resolves
        let png_ct: ContentType = "image/png".parse().expect("valid content type");
        let mut related = MultiPart::related()
            .multipart(alternative)
            .singlepart(
                Attachment::new_inline(LOGO_CID.to_string())
                    .body(Body::new(LOGO_BYTES.to_vec()), png_ct.clone()),
            );

        // Attach any additional inline CID images (e.g. rendered charts)
        for (cid, png_bytes) in images {
            related = related.singlepart(
                Attachment::new_inline(cid.clone())
                    .body(Body::new(png_bytes.clone()), png_ct.clone()),
            );
        }

        let mut builder = Message::builder()
            .from(from_mailbox)
            .to(to_mailbox)
            .subject(subject);

        // Set Reply-To header if provided
        if let Some(reply_to_addr) = reply_to {
            match reply_to_addr.parse::<Mailbox>() {
                Ok(mb) => builder = builder.reply_to(mb),
                Err(e) => {
                    tracing::warn!("Failed to parse Reply-To address '{}': {}", reply_to_addr, e);
                    // Continue without Reply-To — don't fail the email
                }
            }
        }

        let message = match builder.multipart(related) {
            Ok(msg) => msg,
            Err(e) => {
                tracing::error!(to = %to_email, "Failed to build email message: {}", e);
                return false;
            }
        };

        let creds = Credentials::new(smtp_user.to_string(), smtp_password.to_string());

        // Build the SMTP transport. Configuration errors (bad hostname, invalid
        // credentials format) are not retryable — return immediately.
        let mailer_result = if self.smtp_port == 465 {
            AsyncSmtpTransport::<Tokio1Executor>::relay(smtp_host).map(|b| {
                b.port(self.smtp_port).credentials(creds).build()
            })
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(smtp_host).map(|b| {
                b.port(self.smtp_port).credentials(creds).build()
            })
        };

        let mailer = match mailer_result {
            Ok(m) => m,
            Err(e) => {
                tracing::error!(to = %to_email, "Failed to create SMTP transport: {}", e);
                return false;
            }
        };

        // Retry transient SMTP errors (4xx SMTP codes — transient deferrals —
        // network timeouts, connection drops). Permanent errors (5xx SMTP codes
        // — hard rejections — and auth failures) are not retried: they will
        // produce the same result on every attempt.
        let send_result = tane_core::retry::retry_with_backoff_classified(
            || {
                let mailer = mailer.clone();
                let message = message.clone();
                async move { mailer.send(message).await }
            },
            |e: &lettre::transport::smtp::Error| {
                e.is_transient() || e.is_timeout()
            },
        )
        .await;

        match send_result {
            Ok(_) => {
                tracing::info!(to = %to_email, subject = %subject, "Email sent successfully");
                true
            }
            Err(e) => {
                tracing::error!(to = %to_email, "Failed to send email: {}", e);
                false
            }
        }
    }

    /// Send a workspace invitation email.
    ///
    /// Returns `true` if sent successfully.
    pub async fn send_workspace_invitation(
        &self,
        email: &str,
        workspace_name: &str,
        inviter_name: &str,
        role: &str,
    ) -> bool {
        let role_display = if role == "admin" {
            "an Admin"
        } else {
            "a Member"
        };

        let frontend_url = &self.frontend_url;
        let subject = format!("You've been invited to join {} on Tane", workspace_name);

        let escaped_email = html_escape(email);
        let escaped_inviter = html_escape(inviter_name);
        let escaped_workspace = html_escape(workspace_name);

        let body_html = format!(
            r#"<h1>You're Invited!</h1>

        <p><strong>{escaped_inviter}</strong> has invited you to join <strong>{escaped_workspace}</strong> as {role_display}.</p>

        <p>To accept this invitation, sign in with the email address this was sent to ({escaped_email}).</p>

        <div class="cta">
            <a href="{frontend_url}/login" class="button">Accept Invitation</a>
        </div>

        <p>This invitation will expire in 7 days.</p>"#
        );

        let footer_html = format!(
            r#"<p style="margin: 0 0 8px 0;">
            You're receiving this because you were invited to join a workspace on Tane.
        </p>
        <p style="margin: 0;">
            <a href="{frontend_url}/unsubscribe?email={escaped_email}">Unsubscribe</a> &middot;
            <a href="{frontend_url}/privacy">Privacy</a> &middot;
            <a href="{frontend_url}/terms">Terms</a> &middot;
            <a href="{frontend_url}" style="color: #4f46e5;">tane.ai</a>
        </p>"#
        );

        let html_body = email_template(frontend_url, &body_html, &footer_html);

        let text_body = format!(
            "\
You're Invited!

{inviter_name} has invited you to join {workspace_name} as {role_display}.

To accept, sign in with this email address ({email}):
{frontend_url}/login

This invitation will expire in 7 days.
",
        );

        self.send_email(email, &subject, &html_body, Some(&text_body), None, &[])
            .await
    }

    /// Send an ownership transfer notification email.
    ///
    /// `variant` is either "initiated" (sent to recipient) or "confirmation" (sent to current owner).
    pub async fn send_ownership_transfer(
        &self,
        email: &str,
        workspace_name: &str,
        from_name: &str,
        to_name: &str,
        variant: &str,
    ) -> bool {
        let (subject, body_text) = if variant == "initiated" {
            (
                format!("You've been offered ownership of {workspace_name}"),
                format!(
                    "{from_name} wants to transfer ownership of {workspace_name} to you. \
                     Log in to review and accept or decline this transfer."
                ),
            )
        } else {
            (
                format!("Ownership transfer initiated for {workspace_name}"),
                format!(
                    "You initiated an ownership transfer of {workspace_name} to {to_name}. \
                     They have 7 days to accept. You can cancel this transfer from workspace settings."
                ),
            )
        };

        let html_body = format!(
            r#"
        <h1>{subject}</h1>
        <p>{body_text}</p>
        <div class="cta">
            <a href="{frontend_url}/settings/team" class="button">View in Settings</a>
        </div>
"#,
            subject = subject,
            body_text = body_text,
            frontend_url = self.frontend_url,
        );

        let text_body = format!("{subject}\n\n{body_text}\n\nView: {}/settings/team\n", self.frontend_url);

        self.send_email(email, &subject, &html_body, Some(&text_body), None, &[])
            .await
    }

    /// Send a passkey recovery email.
    ///
    /// Returns `true` if sent successfully.
    pub async fn send_passkey_recovery(
        &self,
        email: &str,
        name: &str,
        recovery_link: &str,
    ) -> bool {
        let display_name = if name.is_empty() { "there" } else { name };
        let frontend_url = &self.frontend_url;
        let subject = "Recover your Tane account";

        let escaped_frontend = html_escape(frontend_url);
        let escaped_name = html_escape(display_name);
        let escaped_link = html_escape(recovery_link);

        let body_html = format!(
            r#"<h1>Recover Your Account</h1>

        <p>Hi {escaped_name},</p>

        <p>Click the button below to recover your account and create a new passkey:</p>

        <div class="cta">
            <a href="{escaped_link}" class="button">Create New Passkey</a>
        </div>

        <p style="color: #e74c3c; font-size: 14px;"><strong>This link expires in 15 minutes and can only be used once.</strong></p>

        <p>If you didn't request this, please ignore this email. Your account is secure—no changes have been made.</p>

        <p>Thanks,<br>The Tane Team</p>"#
        );

        let footer_html = format!(
            r#"<p style="margin: 0 0 8px 0;">
            You're receiving this because you requested account recovery for Tane.
        </p>
        <p style="margin: 0;">
            <a href="{escaped_frontend}" style="color: #4f46e5;">tane.ai</a>
        </p>"#
        );

        let html_body = email_template(&escaped_frontend, &body_html, &footer_html);

        let text_body = format!(
            "\
Recover Your Account

Hi {display_name},

Click the link below to recover your account and create a new passkey:

{recovery_link}

IMPORTANT: This link expires in 15 minutes and can only be used once.

If you didn't request this, please ignore this email. Your account is secure\u{2014}no changes have been made.

Thanks,
The Tane Team

---
You're receiving this email because you requested account recovery for Tane.
{frontend_url}
",
        );

        self.send_email(email, subject, &html_body, Some(&text_body), None, &[])
            .await
    }

    /// Send an account recovery email.
    ///
    /// Returns `true` if sent successfully.
    pub async fn send_account_recovery(
        &self,
        email: &str,
        name: &str,
        recovery_link: &str,
    ) -> bool {
        let display_name = if name.is_empty() { "there" } else { name };
        let frontend_url = &self.frontend_url;
        let subject = "Recover your Tane account";

        let escaped_frontend = html_escape(frontend_url);
        let escaped_name = html_escape(display_name);
        let escaped_link = html_escape(recovery_link);

        let body_html = format!(
            r#"<h1>Recover Your Account</h1>

        <p>Hi {escaped_name},</p>

        <p>Click the button below to recover your account and set a new password:</p>

        <div class="cta">
            <a href="{escaped_link}" class="button">Recover Account</a>
        </div>

        <p style="color: #e74c3c; font-size: 14px;"><strong>This link expires in 15 minutes and can only be used once.</strong></p>

        <p>If you didn't request this, please ignore this email. Your account is secure—no changes have been made.</p>

        <p>Thanks,<br>The Tane Team</p>"#
        );

        let footer_html = format!(
            r#"<p style="margin: 0 0 8px 0;">
            You're receiving this because you requested account recovery for Tane.
        </p>
        <p style="margin: 0;">
            <a href="{escaped_frontend}" style="color: #4f46e5;">tane.ai</a>
        </p>"#
        );

        let html_body = email_template(&escaped_frontend, &body_html, &footer_html);

        let text_body = format!(
            "\
Recover Your Account

Hi {display_name},

Click the link below to recover your account and set a new password:

{recovery_link}

IMPORTANT: This link expires in 15 minutes and can only be used once.

If you didn't request this, please ignore this email. Your account is secure\u{2014}no changes have been made.

Thanks,
The Tane Team

---
You're receiving this email because you requested account recovery for Tane.
{frontend_url}
",
        );

        self.send_email(email, subject, &html_body, Some(&text_body), None, &[])
            .await
    }

    /// Send a verification email for account signup.
    ///
    /// Returns `true` if sent successfully.
    pub async fn send_verification_email(
        &self,
        email: &str,
        name: &str,
        verification_link: &str,
    ) -> bool {
        let display_name = if name.is_empty() { "there" } else { name };
        let frontend_url = &self.frontend_url;
        let subject = "Verify your Tane account";

        let escaped_frontend = html_escape(frontend_url);
        let escaped_name = html_escape(display_name);
        let escaped_link = html_escape(verification_link);

        let body_html = format!(
            r#"<h1>Verify Your Email</h1>

        <p>Hi {escaped_name},</p>

        <p>Thanks for signing up for Tane! Click the button below to verify your email address and complete your account setup:</p>

        <div class="cta">
            <a href="{escaped_link}" class="button">Verify Email Address</a>
        </div>

        <p style="color: #e74c3c; font-size: 14px;"><strong>This link expires in 24 hours.</strong></p>

        <p>If you didn't create a Tane account, please ignore this email.</p>

        <p>Thanks,<br>The Tane Team</p>"#
        );

        let footer_html = format!(
            r#"<p style="margin: 0 0 8px 0;">
            You're receiving this because someone signed up for Tane with this email address.
        </p>
        <p style="margin: 0;">
            <a href="{escaped_frontend}" style="color: #4f46e5;">tane.ai</a>
        </p>"#
        );

        let html_body = email_template(&escaped_frontend, &body_html, &footer_html);

        let text_body = format!(
            "\
Verify Your Email

Hi {display_name},

Thanks for signing up for Tane! Click the link below to verify your email address and complete your account setup:

{verification_link}

IMPORTANT: This link expires in 24 hours.

If you didn't create a Tane account, please ignore this email.

Thanks,
The Tane Team

---
You're receiving this because someone signed up for Tane with this email address.
{frontend_url}
",
        );

        self.send_email(email, subject, &html_body, Some(&text_body), None, &[])
            .await
    }

    /// Send a welcome email to a new newsletter subscriber.
    ///
    /// Returns `true` if sent successfully.
    pub async fn send_subscription_welcome(
        &self,
        email: &str,
    ) -> bool {
        let frontend_url = &self.frontend_url;
        let subject = "Welcome to Tane!";

        let escaped_email = html_escape(email);

        let body_html = format!(
            r#"<h1>Welcome to Tane!</h1>

        <p>Thanks for signing up! We're excited to have you on board.</p>

        <p>Your team uses Tane to collaborate and get work done together.</p>

        <p>We'll keep you updated on new features and when your account is ready.</p>

        <div class="cta">
            <a href="{frontend_url}" class="button">Visit Tane</a>
        </div>

        <p>Thanks,<br>The Tane Team</p>"#
        );

        let footer_html = format!(
            r#"<p style="margin: 0 0 8px 0;">
            You're receiving this because you signed up for updates from Tane.
        </p>
        <p style="margin: 0;">
            <a href="{frontend_url}/unsubscribe?email={escaped_email}">Unsubscribe</a> &middot;
            <a href="{frontend_url}" style="color: #4f46e5;">tane.ai</a>
        </p>"#
        );

        let html_body = email_template(frontend_url, &body_html, &footer_html);

        let text_body = format!(
            "\
Welcome to Tane!

Thanks for signing up! We're excited to have you on board.

Your team uses Tane to collaborate and get work done together.

We'll keep you updated on new features and when your account is ready.

Visit Tane: {frontend_url}

Thanks,
The Tane Team

---
You're receiving this because you signed up for updates from Tane.
Unsubscribe: {frontend_url}/unsubscribe?email={email}
{frontend_url}
",
        );

        self.send_email(email, subject, &html_body, Some(&text_body), None, &[])
            .await
    }

    /// Send a plain admin notification email (feedback alerts, signup alerts).
    ///
    /// Uses minimal styling — these are internal notifications, not user-facing emails.
    /// `reply_to` sets the Reply-To header so support can reply directly to the user.
    pub async fn send_admin_notification(
        &self,
        to_email: &str,
        subject: &str,
        sections: &[(& str, &str)],
        reply_to: Option<&str>,
    ) -> bool {
        let frontend_url = &self.frontend_url;

        // Build HTML sections
        let html_sections: String = sections
            .iter()
            .map(|(label, value)| {
                format!(
                    r#"<tr><td style="padding:4px 12px 4px 0;font-weight:600;vertical-align:top;white-space:nowrap;">{}</td><td style="padding:4px 0;">{}</td></tr>"#,
                    html_escape(label),
                    html_escape(value),
                )
            })
            .collect();

        let escaped_subject = html_escape(subject);
        let html_body = admin_email_template(frontend_url, &escaped_subject, &html_sections);

        // Build text sections
        let text_sections: String = sections
            .iter()
            .map(|(label, value)| format!("{label}: {value}"))
            .collect::<Vec<_>>()
            .join("\n");

        let text_body = format!("{subject}\n\n{text_sections}\n\n---\ntane.ai\n");

        self.send_email(to_email, subject, &html_body, Some(&text_body), reply_to, &[])
            .await
    }
}

/// Produce a full HTML email document with the shared Tane branding.
///
/// `frontend_url` — base URL used for logo link and footer links.
/// `body_html`    — unique content inserted inside `<div class="content">…</div>`.
/// `footer_html`  — content inserted inside `<div class="footer">…</div>`.
///
/// The returned string includes `<!DOCTYPE html>`, the shared `<style>` block with
/// all CSS classes used by any user-facing template (including dark-mode media query),
/// the CID-referenced logo header, and all closing tags.
fn email_template(frontend_url: &str, body_html: &str, footer_html: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <meta name="color-scheme" content="light dark">
    <meta name="supported-color-schemes" content="light dark">
    <style>
        :root {{ color-scheme: light dark; }}
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif;
            line-height: 1.6;
            color: #1C1917;
            max-width: 600px;
            margin: 0 auto;
            padding: 20px;
            background-color: #FAFAF8;
        }}
        .header {{
            text-align: center;
            margin-bottom: 16px;
            padding: 16px 0;
            border-bottom: 1px solid #E8E5DE;
        }}
        .logo-img {{
            height: 48px;
            width: auto;
        }}
        .content {{
            padding: 20px 0;
        }}
        h1 {{
            color: #1C1917;
            font-size: 24px;
            font-weight: 700;
            margin-bottom: 16px;
        }}
        h2 {{
            color: #1C1917;
            font-size: 20px;
            font-weight: 600;
            margin: 24px 0 12px 0;
        }}
        h3 {{
            color: #1C1917;
            font-size: 18px;
            font-weight: 600;
            margin: 20px 0 10px 0;
        }}
        p {{
            color: #6B6660;
            font-size: 14px;
            margin: 12px 0;
        }}
        .highlight {{
            background-color: #fffbeb;
            border-left: 4px solid #4f46e5;
            padding: 16px;
            margin: 24px 0;
            border-radius: 0 8px 8px 0;
        }}
        .cta {{
            text-align: center;
            margin: 32px 0;
        }}
        .button {{
            display: inline-block;
            background-color: #4f46e5;
            color: #ffffff !important;
            padding: 14px 28px;
            text-decoration: none;
            border-radius: 8px;
            font-weight: 600;
            font-size: 14px;
        }}
        .footer {{
            margin-top: 20px;
            padding-top: 16px;
            border-top: 1px solid #E8E5DE;
            text-align: center;
            color: #9C9790;
            font-size: 12px;
        }}
        .footer a {{
            color: #6B6660;
            text-decoration: none;
        }}
        .footer a:hover {{
            text-decoration: underline;
        }}
        @media (prefers-color-scheme: dark) {{
            body {{ background-color: #12100F !important; color: #F5F3EF !important; }}
            h1, h2, h3 {{ color: #F5F3EF !important; }}
            p {{ color: #A8A29E !important; }}
            .header {{ border-bottom-color: #2E2925 !important; }}
            .highlight {{ background-color: #2C241E !important; }}
            .feature {{ color: #A8A29E !important; }}
            .footer {{ border-top-color: #2E2925 !important; color: #78716C !important; }}
            .footer a {{ color: #A8A29E !important; }}
        }}
    </style>
</head>
<body style="background-color: #FAFAF8; color: #1C1917;">
    <div class="header">
        <a href="{frontend_url}" style="text-decoration: none;">
            <img src="cid:tane_logo" alt="Tane" class="logo-img" style="height: 48px; width: auto;">
        </a>
    </div>
    <div class="content">
        {body_html}
    </div>
    <div class="footer">
        {footer_html}
    </div>
</body>
</html>"#
    )
}

/// Produce a full HTML email document for internal admin notifications.
///
/// Uses a distinct, minimal template: table layout for key-value sections,
/// no `.content` wrapper, and `td`-specific dark-mode rules.
fn admin_email_template(frontend_url: &str, subject: &str, sections_html: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <meta name="color-scheme" content="light dark">
    <meta name="supported-color-schemes" content="light dark">
    <style>
        :root {{ color-scheme: light dark; }}
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            line-height: 1.6;
            color: #1C1917;
            max-width: 600px;
            margin: 0 auto;
            padding: 20px;
            background-color: #FAFAF8;
        }}
        .header {{
            text-align: center;
            margin-bottom: 16px;
            padding: 16px 0;
            border-bottom: 1px solid #E8E5DE;
        }}
        h2 {{
            color: #1C1917;
        }}
        td {{
            color: #6B6660;
        }}
        td:first-child {{
            color: #1C1917;
        }}
        .footer {{
            margin-top: 20px;
            padding-top: 16px;
            border-top: 1px solid #E8E5DE;
            text-align: center;
            color: #9C9790;
            font-size: 12px;
        }}
        @media (prefers-color-scheme: dark) {{
            body {{ background-color: #12100F !important; color: #F5F3EF !important; }}
            h2 {{ color: #F5F3EF !important; }}
            .header {{ border-bottom-color: #2E2925 !important; }}
            .footer {{ border-top-color: #2E2925 !important; color: #78716C !important; }}
            .footer a {{ color: #A8A29E !important; }}
            td {{ color: #A8A29E !important; }}
            td:first-child {{ color: #F5F3EF !important; }}
        }}
    </style>
</head>
<body>
    <div class="header">
        <a href="{frontend_url}" style="text-decoration: none;">
            <img src="cid:tane_logo" alt="Tane" style="height: 48px; width: auto;">
        </a>
    </div>
    <h2 style="margin:0 0 16px 0;">{subject}</h2>
    <table style="border-collapse:collapse;width:100%;font-size:14px;">
        {sections_html}
    </table>
    <div class="footer">
        <p style="margin:0;"><a href="{frontend_url}" style="color: #4f46e5;">tane.ai</a></p>
    </div>
</body>
</html>"#
    )
}

/// HTML escaping for user-provided strings inserted into email templates.
///
/// Covers the OWASP-recommended set: &, <, >, ", ', /, and backtick.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
        .replace('/', "&#x2F;")
        .replace('`', "&#96;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_service_not_configured_by_default() {
        // Without SMTP env vars, service should report not configured.
        // This test is safe because CI/dev environments don't set SMTP vars.
        let service = EmailService {
            smtp_host: None,
            smtp_port: 587,
            smtp_user: None,
            smtp_password: None,
            from_email: "noreply@tane.ai".to_string(),
            from_name: "Tane".to_string(),
            frontend_url: "https://app.tane.ai".to_string(),
        };
        assert!(!service.is_configured());
    }

    #[test]
    fn email_service_configured_when_all_vars_set() {
        let service = EmailService {
            smtp_host: Some("smtp.example.com".to_string()),
            smtp_port: 587,
            smtp_user: Some("user@example.com".to_string()),
            smtp_password: Some("password".to_string()),
            from_email: "noreply@tane.ai".to_string(),
            from_name: "Tane".to_string(),
            frontend_url: "https://app.tane.ai".to_string(),
        };
        assert!(service.is_configured());
    }

    #[test]
    fn html_escape_works() {
        assert_eq!(html_escape("<script>"), "&lt;script&gt;");
        assert_eq!(html_escape("A & B"), "A &amp; B");
        assert_eq!(html_escape("\"hello\""), "&quot;hello&quot;");
        assert_eq!(html_escape("a/b"), "a&#x2F;b");
        assert_eq!(html_escape("a`b"), "a&#96;b");
        assert_eq!(html_escape("safe text 123"), "safe text 123");
    }

    #[tokio::test]
    async fn send_email_returns_false_when_not_configured() {
        let service = EmailService {
            smtp_host: None,
            smtp_port: 587,
            smtp_user: None,
            smtp_password: None,
            from_email: "noreply@tane.ai".to_string(),
            from_name: "Tane".to_string(),
            frontend_url: "https://app.tane.ai".to_string(),
        };

        let result = service
            .send_email("test@example.com", "Test", "<p>Hi</p>", None, None, &[])
            .await;
        assert!(!result);
    }

    #[tokio::test]
    async fn send_workspace_invitation_returns_false_when_not_configured() {
        let service = EmailService {
            smtp_host: None,
            smtp_port: 587,
            smtp_user: None,
            smtp_password: None,
            from_email: "noreply@tane.ai".to_string(),
            from_name: "Tane".to_string(),
            frontend_url: "https://app.tane.ai".to_string(),
        };

        let result = service
            .send_workspace_invitation("test@example.com", "My Workspace", "Jane Doe", "admin")
            .await;
        assert!(!result);
    }
}
