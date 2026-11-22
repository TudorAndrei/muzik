use serde::{Deserialize, Serialize};
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        href, page, query_params, route,
    },
    view::{View, view},
};

use crate::{auth, ui::Bare};

#[query_params(error = bad_request)]
struct LoginQuery {
    failed: Option<String>,
}

#[derive(Serialize)]
struct Failed {
    failed: &'static str,
}

#[derive(Deserialize)]
struct LoginForm {
    token: String,
}

#[page("/login", with(Bare))]
pub async fn login(cx: &Cx) -> Result<impl View> {
    let failed = query_params::<LoginQuery>(cx)?.failed.is_some();
    Ok(view! {
        <div class="login card">
            <h1>"muzik"</h1>
            <form method="post" action=(href!(login_submit))>
                <p class="notice error">
                    if failed {
                        "The access token is not valid."
                    }
                </p>
                <input
                    type="password"
                    name="token"
                    placeholder="Access token"
                    autocomplete="current-password"
                    required="true"
                    autofocus="true"
                >
                <button class="primary" type="submit">"Sign in"</button>
            </form>
        </div>
    })
}

#[route(POST "/login")]
pub async fn login_submit(cx: &Cx, Form(form): Form<LoginForm>) -> Result<SeeOther> {
    let accepted = !auth::auth(cx).required() || auth::login(cx, &form.token).await?;
    let target = if accepted {
        href!(crate::activity::page).resolve(cx)
    } else {
        href!(login).query(&Failed { failed: "1" }).resolve(cx)
    };
    Ok(see_other(target))
}

#[route(POST "/logout")]
pub async fn logout(cx: &Cx) -> Result<SeeOther> {
    auth::logout(cx).await?;
    Ok(see_other(href!(login).resolve(cx)))
}

#[page("/library")]
pub async fn library(cx: &Cx) -> Result<impl View> {
    auth::require(cx).await?;
    Ok(view! {
        <h1>"Library"</h1>
        <p class="placeholder">"The library is not available in the web app yet."</p>
    })
}

#[page("/settings")]
pub async fn settings(cx: &Cx) -> Result<impl View> {
    auth::require(cx).await?;
    Ok(view! {
        <h1>"Settings"</h1>
        <p class="placeholder">"The settings are not available in the web app yet."</p>
    })
}
