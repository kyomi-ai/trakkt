// SPDX-License-Identifier: AGPL-3.0-or-later

//! Billing settings page — Stripe subscription management UI.
//!
//! Displays the workspace's billing state and allows workspace owners to
//! manage their subscription. Four states based on `BillingInfo.subscription_status`:
//!
//! - **Free**: No subscription — show subscribe CTA with plan details.
//! - **Active**: Active subscription — show plan details, invoices, manage button.
//! - **Past Due**: Payment failed — warning banner + "Fix" button (opens Stripe Portal).
//! - **Cancelled**: Cancelling at period end — show reactivate button.
//!
//! Follows the same `Resource::new` + `Transition` + `Suspend` pattern as
//! the integrations page (TRA-93).

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};

use crate::components::{
    Alert, AlertDescription, AlertVariant, Button, ButtonLink, ButtonSize, ButtonVariant, Card,
    CardContent, CardHeader, CardTitle, Skeleton, Spinner,
};
use crate::server_fns::billing::{
    cancel_billing_subscription, create_billing_portal_session, create_checkout,
    get_billing_info, get_billing_invoices, reactivate_billing_subscription, BillingInfo,
    InvoiceInfo,
};

// ─────────────────────────────────────────────────────────────────────────────
// Portal redirect helper
// ─────────────────────────────────────────────────────────────────────────────

/// Create an action + effect pair that opens the Stripe billing portal.
/// Returns the action (to dispatch) and a loading signal.
fn use_portal_redirect(
    on_error: WriteSignal<Option<String>>,
) -> (Action<(), Result<String, ServerFnError>>, Signal<bool>) {
    let portal_action = Action::new(move |_: &()| async move {
        create_billing_portal_session().await
    });

    Effect::new(move || {
        if let Some(result) = portal_action.value().get() {
            match result {
                Ok(url) => {
                    #[cfg(target_arch = "wasm32")]
                    {
                        if let Some(window) = web_sys::window() {
                            let _ = window.location().set_href(&url);
                        }
                    }
                    let _ = url;
                }
                Err(e) => {
                    on_error.set(Some(e.to_string()));
                }
            }
        }
    });

    let is_loading = Signal::derive(move || portal_action.pending().get());
    (portal_action, is_loading)
}

// ─────────────────────────────────────────────────────────────────────────────
// Main page
// ─────────────────────────────────────────────────────────────────────────────

#[component]
pub fn BillingPage() -> impl IntoView {
    let (version, set_version) = signal(0u32);
    let billing_resource = Resource::new(move || version.get(), |_| get_billing_info());

    view! {
        <div class="p-4 sm:p-6">
            <h2 class="text-xl font-display text-foreground mb-4">"Billing"</h2>
            <p class="text-muted-foreground mb-6">
                "Manage your workspace subscription and billing."
            </p>

            <Transition fallback=move || view! {
                <Card>
                    <CardHeader>
                        <Skeleton class="h-5 w-1/3"/>
                    </CardHeader>
                    <CardContent>
                        <div class="space-y-3">
                            <Skeleton class="h-4 w-2/3"/>
                            <Skeleton class="h-4 w-1/2"/>
                            <Skeleton class="h-10 w-40"/>
                        </div>
                    </CardContent>
                </Card>
            }>
                {move || Suspend::new(async move {
                    match billing_resource.await {
                        Ok(Some(info)) => {
                            let on_change = Callback::new(move |()| {
                                set_version.update(|v| *v += 1);
                            });
                            match info.subscription_status.as_str() {
                                "active" | "trialing" => {
                                    view! { <ActivePlanCard info=info on_change=on_change/> }.into_any()
                                }
                                "past_due" => {
                                    view! { <PastDueCard info=info/> }.into_any()
                                }
                                "cancelled" | "canceled" => {
                                    view! { <CancelledCard info=info on_change=on_change/> }.into_any()
                                }
                                _ => {
                                    // "free" or any unknown status
                                    view! { <FreePlanCard info=info/> }.into_any()
                                }
                            }
                        }
                        Ok(None) => {
                            // Billing not enabled (self-hosted)
                            view! {
                                <Card>
                                    <CardHeader>
                                        <div class="flex items-center gap-2">
                                            <Icon icon=phosphor_leptos::CREDIT_CARD weight=IconWeight::Regular size="24px" attr:class="text-muted-foreground"/>
                                            <CardTitle>"Billing"</CardTitle>
                                        </div>
                                    </CardHeader>
                                    <CardContent>
                                        <Alert variant=AlertVariant::Info>
                                            <AlertDescription>
                                                "Billing is not enabled for this instance. Self-hosted installations do not require a subscription."
                                            </AlertDescription>
                                        </Alert>
                                    </CardContent>
                                </Card>
                            }.into_any()
                        }
                        Err(e) => {
                            let msg = e.to_string();
                            view! {
                                <Card>
                                    <div class="p-6">
                                        <Alert variant=AlertVariant::Error>
                                            <AlertDescription>
                                                "Failed to load billing information: " {msg}
                                            </AlertDescription>
                                        </Alert>
                                    </div>
                                </Card>
                            }.into_any()
                        }
                    }
                })}
            </Transition>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// State: Free — no subscription, show subscribe CTA
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn FreePlanCard(info: BillingInfo) -> impl IntoView {
    let (checkout_error, set_checkout_error) = signal(Option::<String>::None);

    let checkout_action = Action::new(move |quantity: &u64| {
        let quantity = *quantity;
        async move { create_checkout(quantity).await }
    });

    let (portal_action, is_portal_loading) = use_portal_redirect(set_checkout_error);

    Effect::new(move || {
        if let Some(result) = checkout_action.value().get() {
            match result {
                Ok(_checkout_result) => {
                    portal_action.dispatch(());
                }
                Err(e) => {
                    set_checkout_error.set(Some(e.to_string()));
                }
            }
        }
    });

    let is_loading = Signal::derive(move || {
        checkout_action.pending().get() || is_portal_loading.get()
    });

    let user_count = info.user_count;

    view! {
        <div class="space-y-6">
            // Current plan
            <Card>
                <CardHeader>
                    <div class="flex items-center gap-2">
                        <Icon icon=phosphor_leptos::CREDIT_CARD weight=IconWeight::Regular size="24px" attr:class="text-muted-foreground"/>
                        <CardTitle>"Current Plan"</CardTitle>
                    </div>
                </CardHeader>
                <CardContent>
                    <div class="space-y-4">
                        <div class="flex items-center gap-3">
                            <span class="text-sm font-medium text-foreground">"Plan:"</span>
                            <span class="text-sm text-secondary-foreground">"Free (solo)"</span>
                        </div>
                        <p class="text-sm text-muted-foreground">
                            "You are on the free plan. To invite team members and collaborate, subscribe to the Team plan."
                        </p>
                    </div>
                </CardContent>
            </Card>

            // Team plan CTA
            <Card>
                <CardHeader>
                    <CardTitle>"Team Plan"</CardTitle>
                </CardHeader>
                <CardContent>
                    <div class="space-y-4">
                        <div class="flex items-baseline gap-1">
                            <span class="text-2xl font-display text-foreground">"$5"</span>
                            <span class="text-sm text-muted-foreground">"/ user / month"</span>
                        </div>

                        <ul class="text-sm text-secondary-foreground space-y-2">
                            <li class="flex items-center gap-2">
                                <Icon icon=phosphor_leptos::CHECK weight=IconWeight::Bold size="16px" attr:class="text-primary flex-shrink-0"/>
                                "Unlimited team members"
                            </li>
                            <li class="flex items-center gap-2">
                                <Icon icon=phosphor_leptos::CHECK weight=IconWeight::Bold size="16px" attr:class="text-primary flex-shrink-0"/>
                                "Role-based access control"
                            </li>
                            <li class="flex items-center gap-2">
                                <Icon icon=phosphor_leptos::CHECK weight=IconWeight::Bold size="16px" attr:class="text-primary flex-shrink-0"/>
                                "Priority support"
                            </li>
                        </ul>

                        // Error display
                        {move || checkout_error.get().map(|e| view! {
                            <Alert variant=AlertVariant::Error>
                                <AlertDescription>{e}</AlertDescription>
                            </Alert>
                        })}

                        <Button
                            variant=ButtonVariant::Default
                            disabled=MaybeProp::from(is_loading)
                            on:click=move |_| {
                                set_checkout_error.set(None);
                                let qty = std::cmp::max(user_count as u64, 1);
                                checkout_action.dispatch(qty);
                            }
                        >
                            {move || {
                                if is_loading.get() {
                                    view! {
                                        <Spinner class="text-primary-foreground"/>
                                        "Setting up..."
                                    }.into_any()
                                } else {
                                    view! {
                                        <Icon icon=phosphor_leptos::CREDIT_CARD weight=IconWeight::Bold size="16px"/>
                                        "Subscribe \u{2014} $5/user/month"
                                    }.into_any()
                                }
                            }}
                        </Button>
                    </div>
                </CardContent>
            </Card>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// State: Active — show plan details, invoices, manage button
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn ActivePlanCard(info: BillingInfo, on_change: Callback<()>) -> impl IntoView {
    let (action_error, set_action_error) = signal(Option::<String>::None);
    let (show_cancel_confirm, set_show_cancel_confirm) = signal(false);

    let cancel_action = Action::new(move |_: &()| async move {
        cancel_billing_subscription().await
    });

    let (portal_action, is_portal_loading) = use_portal_redirect(set_action_error);

    Effect::new(move || {
        if let Some(result) = cancel_action.value().get() {
            match result {
                Ok(()) => {
                    set_show_cancel_confirm.set(false);
                    set_action_error.set(None);
                    on_change.run(());
                }
                Err(e) => {
                    set_action_error.set(Some(e.to_string()));
                }
            }
        }
    });

    let is_cancelling = Signal::derive(move || cancel_action.pending().get());

    let monthly_cost = info.monthly_cost.map(|c| format!("${:.2}", c)).unwrap_or_else(|| "$0.00".to_string());
    let user_count = info.user_count;
    let period_end = info.period_end.clone().map(|dt| format_period_end(&dt)).unwrap_or_else(|| "\u{2014}".to_string());

    view! {
        <div class="space-y-6">
            // Active plan details
            <Card>
                <CardHeader>
                    <div class="flex items-center justify-between">
                        <div class="flex items-center gap-2">
                            <Icon icon=phosphor_leptos::CREDIT_CARD weight=IconWeight::Regular size="24px" attr:class="text-muted-foreground"/>
                            <CardTitle>"Team Plan"</CardTitle>
                        </div>
                        <div class="flex items-center gap-1.5">
                            <Icon icon=phosphor_leptos::CHECK_CIRCLE weight=IconWeight::Fill size="16px" attr:class="text-success-foreground"/>
                            <span class="text-sm font-medium text-success-foreground">"Active"</span>
                        </div>
                    </div>
                </CardHeader>
                <CardContent>
                    <div class="space-y-4">
                        // Error display
                        {move || action_error.get().map(|e| view! {
                            <Alert variant=AlertVariant::Error>
                                <AlertDescription>{e}</AlertDescription>
                            </Alert>
                        })}

                        // Plan metrics
                        <div class="grid grid-cols-1 sm:grid-cols-3 gap-4">
                            <div class="space-y-1">
                                <span class="text-xs text-muted-foreground uppercase tracking-wider">"Seats"</span>
                                <div class="text-lg font-medium text-foreground">{user_count}</div>
                            </div>
                            <div class="space-y-1">
                                <span class="text-xs text-muted-foreground uppercase tracking-wider">"Monthly Cost"</span>
                                <div class="text-lg font-medium text-foreground">{monthly_cost}</div>
                            </div>
                            <div class="space-y-1">
                                <span class="text-xs text-muted-foreground uppercase tracking-wider">"Next Billing"</span>
                                <div class="text-lg font-medium text-foreground">{period_end}</div>
                            </div>
                        </div>

                        <div class="border-t border-border pt-4 mt-4 flex flex-wrap items-center gap-3">
                            // Manage billing (opens Stripe Portal)
                            <Button
                                variant=ButtonVariant::Outline
                                size=ButtonSize::Sm
                                disabled=MaybeProp::from(is_portal_loading)
                                on:click=move |_| {
                                    set_action_error.set(None);
                                    portal_action.dispatch(());
                                }
                            >
                                {move || {
                                    if is_portal_loading.get() {
                                        view! {
                                            <Spinner/>
                                            "Opening..."
                                        }.into_any()
                                    } else {
                                        view! {
                                            <Icon icon=phosphor_leptos::ARROW_SQUARE_OUT weight=IconWeight::Light size="14px"/>
                                            "Manage Billing"
                                        }.into_any()
                                    }
                                }}
                            </Button>

                            // Cancel subscription
                            {move || {
                                if show_cancel_confirm.get() {
                                    view! {
                                        <div class="flex items-center gap-2">
                                            <span class="text-xs text-muted-foreground">
                                                "Cancel at end of billing period?"
                                            </span>
                                            <Button
                                                variant=ButtonVariant::Outline
                                                size=ButtonSize::Sm
                                                on:click=move |_| set_show_cancel_confirm.set(false)
                                                disabled=MaybeProp::from(is_cancelling)
                                            >
                                                "Keep"
                                            </Button>
                                            <Button
                                                variant=ButtonVariant::Destructive
                                                size=ButtonSize::Sm
                                                disabled=MaybeProp::from(is_cancelling)
                                                on:click=move |_| { cancel_action.dispatch(()); }
                                            >
                                                {move || {
                                                    if is_cancelling.get() {
                                                        view! {
                                                            <Spinner class="text-white"/>
                                                            "Cancelling..."
                                                        }.into_any()
                                                    } else {
                                                        view! { "Yes, cancel" }.into_any()
                                                    }
                                                }}
                                            </Button>
                                        </div>
                                    }.into_any()
                                } else {
                                    view! {
                                        <Button
                                            variant=ButtonVariant::GhostDestructive
                                            size=ButtonSize::Sm
                                            on:click=move |_| {
                                                set_action_error.set(None);
                                                set_show_cancel_confirm.set(true);
                                            }
                                        >
                                            "Cancel Subscription"
                                        </Button>
                                    }.into_any()
                                }
                            }}
                        </div>
                    </div>
                </CardContent>
            </Card>

            // Invoice history
            <InvoiceHistoryCard/>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// State: Past Due — warning banner + fix button
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn PastDueCard(info: BillingInfo) -> impl IntoView {
    let (action_error, set_action_error) = signal(Option::<String>::None);

    let (portal_action, is_portal_loading) = use_portal_redirect(set_action_error);

    let monthly_cost = info.monthly_cost.map(|c| format!("${:.2}", c)).unwrap_or_else(|| "$0.00".to_string());
    let user_count = info.user_count;
    let period_end = info.period_end.clone().map(|dt| format_period_end(&dt)).unwrap_or_else(|| "\u{2014}".to_string());

    view! {
        <div class="space-y-6">
            // Payment failed warning
            <Alert variant=AlertVariant::Warning>
                <AlertDescription>
                    <div class="flex items-center justify-between gap-4">
                        <div class="flex items-start gap-2">
                            <Icon icon=phosphor_leptos::WARNING weight=IconWeight::Fill size="16px" attr:class="mt-0.5 flex-shrink-0"/>
                            <div>
                                <span class="font-medium">"Payment failed"</span>
                                <span class="text-sm">" \u{2014} team invites are paused until payment is updated."</span>
                            </div>
                        </div>
                        <Button
                            variant=ButtonVariant::Default
                            size=ButtonSize::Sm
                            disabled=MaybeProp::from(is_portal_loading)
                            on:click=move |_| {
                                set_action_error.set(None);
                                portal_action.dispatch(());
                            }
                        >
                            {move || {
                                if is_portal_loading.get() {
                                    view! {
                                        <Spinner class="text-primary-foreground"/>
                                        "Opening..."
                                    }.into_any()
                                } else {
                                    view! { "Update payment method" }.into_any()
                                }
                            }}
                        </Button>
                    </div>
                </AlertDescription>
            </Alert>

            // Plan details (same structure as active)
            <Card>
                <CardHeader>
                    <div class="flex items-center justify-between">
                        <div class="flex items-center gap-2">
                            <Icon icon=phosphor_leptos::CREDIT_CARD weight=IconWeight::Regular size="24px" attr:class="text-muted-foreground"/>
                            <CardTitle>"Team Plan"</CardTitle>
                        </div>
                        <div class="flex items-center gap-1.5">
                            <Icon icon=phosphor_leptos::WARNING weight=IconWeight::Fill size="16px" attr:class="text-warning-foreground"/>
                            <span class="text-sm font-medium text-warning-foreground">"Past Due"</span>
                        </div>
                    </div>
                </CardHeader>
                <CardContent>
                    <div class="space-y-4">
                        {move || action_error.get().map(|e| view! {
                            <Alert variant=AlertVariant::Error>
                                <AlertDescription>{e}</AlertDescription>
                            </Alert>
                        })}

                        <div class="grid grid-cols-1 sm:grid-cols-3 gap-4">
                            <div class="space-y-1">
                                <span class="text-xs text-muted-foreground uppercase tracking-wider">"Seats"</span>
                                <div class="text-lg font-medium text-foreground">{user_count}</div>
                            </div>
                            <div class="space-y-1">
                                <span class="text-xs text-muted-foreground uppercase tracking-wider">"Monthly Cost"</span>
                                <div class="text-lg font-medium text-foreground">{monthly_cost}</div>
                            </div>
                            <div class="space-y-1">
                                <span class="text-xs text-muted-foreground uppercase tracking-wider">"Next Billing"</span>
                                <div class="text-lg font-medium text-foreground">{period_end}</div>
                            </div>
                        </div>
                    </div>
                </CardContent>
            </Card>

            // Invoice history
            <InvoiceHistoryCard/>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// State: Cancelled — reactivate button
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn CancelledCard(info: BillingInfo, on_change: Callback<()>) -> impl IntoView {
    let (action_error, set_action_error) = signal(Option::<String>::None);

    let reactivate_action = Action::new(move |_: &()| async move {
        reactivate_billing_subscription().await
    });

    Effect::new(move || {
        if let Some(result) = reactivate_action.value().get() {
            match result {
                Ok(()) => {
                    set_action_error.set(None);
                    on_change.run(());
                }
                Err(e) => {
                    set_action_error.set(Some(e.to_string()));
                }
            }
        }
    });

    let is_reactivating = Signal::derive(move || reactivate_action.pending().get());

    let period_end = info.period_end.clone().map(|dt| format_period_end(&dt)).unwrap_or_else(|| "\u{2014}".to_string());
    let period_end_display = period_end.clone();
    let user_count = info.user_count;

    view! {
        <div class="space-y-6">
            <Card>
                <CardHeader>
                    <div class="flex items-center justify-between">
                        <div class="flex items-center gap-2">
                            <Icon icon=phosphor_leptos::CREDIT_CARD weight=IconWeight::Regular size="24px" attr:class="text-muted-foreground"/>
                            <CardTitle>"Team Plan"</CardTitle>
                        </div>
                        <div class="flex items-center gap-1.5">
                            <Icon icon=phosphor_leptos::X_CIRCLE weight=IconWeight::Fill size="16px" attr:class="text-muted-foreground"/>
                            <span class="text-sm font-medium text-muted-foreground">"Cancelling"</span>
                        </div>
                    </div>
                </CardHeader>
                <CardContent>
                    <div class="space-y-4">
                        {move || action_error.get().map(|e| view! {
                            <Alert variant=AlertVariant::Error>
                                <AlertDescription>{e}</AlertDescription>
                            </Alert>
                        })}

                        <Alert variant=AlertVariant::Warning>
                            <AlertDescription>
                                <div class="flex items-start gap-2">
                                    <Icon icon=phosphor_leptos::WARNING weight=IconWeight::Bold size="16px" attr:class="mt-0.5 flex-shrink-0"/>
                                    <span>
                                        "Your subscription is set to cancel. You can continue using the Team plan until "
                                        <span class="font-medium">{period_end.clone()}</span>
                                        ". After that, team invites will be disabled."
                                    </span>
                                </div>
                            </AlertDescription>
                        </Alert>

                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                            <div class="space-y-1">
                                <span class="text-xs text-muted-foreground uppercase tracking-wider">"Seats"</span>
                                <div class="text-lg font-medium text-foreground">{user_count}</div>
                            </div>
                            <div class="space-y-1">
                                <span class="text-xs text-muted-foreground uppercase tracking-wider">"Access Until"</span>
                                <div class="text-lg font-medium text-foreground">{period_end_display}</div>
                            </div>
                        </div>

                        <div class="border-t border-border pt-4 mt-4">
                            <Button
                                variant=ButtonVariant::Default
                                disabled=MaybeProp::from(is_reactivating)
                                on:click=move |_| {
                                    set_action_error.set(None);
                                    reactivate_action.dispatch(());
                                }
                            >
                                {move || {
                                    if is_reactivating.get() {
                                        view! {
                                            <Spinner class="text-primary-foreground"/>
                                            "Reactivating..."
                                        }.into_any()
                                    } else {
                                        view! {
                                            <Icon icon=phosphor_leptos::ARROW_COUNTER_CLOCKWISE weight=IconWeight::Bold size="16px"/>
                                            "Reactivate Subscription"
                                        }.into_any()
                                    }
                                }}
                            </Button>
                        </div>
                    </div>
                </CardContent>
            </Card>

            // Invoice history
            <InvoiceHistoryCard/>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Invoice History Card — shared across active/past_due/cancelled states
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn InvoiceHistoryCard() -> impl IntoView {
    let invoices_resource = Resource::new(|| (), |_| get_billing_invoices());

    view! {
        <Card>
            <CardHeader>
                <CardTitle>"Invoice History"</CardTitle>
            </CardHeader>
            <CardContent>
                <Transition fallback=move || view! {
                    <div class="space-y-3">
                        <Skeleton class="h-8 w-full"/>
                        <Skeleton class="h-8 w-full"/>
                        <Skeleton class="h-8 w-full"/>
                    </div>
                }>
                    {move || Suspend::new(async move {
                        match invoices_resource.await {
                            Ok(invoices) if invoices.is_empty() => {
                                view! {
                                    <p class="text-sm text-muted-foreground">"No invoices yet."</p>
                                }.into_any()
                            }
                            Ok(invoices) => {
                                view! {
                                    <div class="overflow-x-auto">
                                        <table class="w-full text-sm">
                                            <thead>
                                                <tr class="border-b border-border">
                                                    <th class="text-left py-2 pr-4 text-xs text-muted-foreground uppercase tracking-wider font-medium">"Date"</th>
                                                    <th class="text-left py-2 pr-4 text-xs text-muted-foreground uppercase tracking-wider font-medium">"Amount"</th>
                                                    <th class="text-left py-2 pr-4 text-xs text-muted-foreground uppercase tracking-wider font-medium">"Status"</th>
                                                    <th class="text-right py-2 text-xs text-muted-foreground uppercase tracking-wider font-medium">"Invoice"</th>
                                                </tr>
                                            </thead>
                                            <tbody>
                                                {invoices.into_iter().map(|invoice| {
                                                    view! { <InvoiceRow invoice=invoice/> }
                                                }).collect_view()}
                                            </tbody>
                                        </table>
                                    </div>
                                }.into_any()
                            }
                            Err(e) => {
                                let msg = e.to_string();
                                view! {
                                    <Alert variant=AlertVariant::Error>
                                        <AlertDescription>
                                            "Failed to load invoices: " {msg}
                                        </AlertDescription>
                                    </Alert>
                                }.into_any()
                            }
                        }
                    })}
                </Transition>
            </CardContent>
        </Card>
    }
}

#[component]
fn InvoiceRow(invoice: InvoiceInfo) -> impl IntoView {
    let status_class = match invoice.status.as_str() {
        "paid" => "text-success-foreground",
        "open" | "draft" => "text-foreground",
        "uncollectible" | "void" => "text-muted-foreground",
        _ => "text-foreground",
    };

    let status_label = match invoice.status.as_str() {
        "paid" => "Paid".to_string(),
        "open" => "Open".to_string(),
        "draft" => "Draft".to_string(),
        "uncollectible" => "Uncollectible".to_string(),
        "void" => "Void".to_string(),
        _ => invoice.status.clone(),
    };

    let pdf_url = invoice.pdf_url.clone();

    view! {
        <tr class="border-b border-border last:border-b-0">
            <td class="py-2.5 pr-4 text-foreground font-mono text-xs">{invoice.date}</td>
            <td class="py-2.5 pr-4 text-foreground">{invoice.amount}</td>
            <td class="py-2.5 pr-4">
                <span class=format!("text-xs font-medium {status_class}")>
                    {status_label}
                </span>
            </td>
            <td class="py-2.5 text-right">
                {pdf_url.map(|url| view! {
                    <ButtonLink
                        href=url
                        target="_blank"
                        rel="noopener noreferrer"
                        variant=ButtonVariant::Ghost
                        size=ButtonSize::Sm
                        class="text-xs"
                    >
                        "PDF"
                        <Icon icon=phosphor_leptos::ARROW_SQUARE_OUT weight=IconWeight::Light size="12px"/>
                    </ButtonLink>
                })}
            </td>
        </tr>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Format an RFC 3339 period end date to a human-readable format (DD/MM/YYYY).
fn format_period_end(rfc3339: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .map(|dt| dt.format("%d/%m/%Y").to_string())
        .unwrap_or_else(|_| {
            // Try just the date portion
            rfc3339
                .split('T')
                .next()
                .unwrap_or(rfc3339)
                .to_string()
        })
}
