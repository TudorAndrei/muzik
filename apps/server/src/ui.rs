use topcoat::{
    Result,
    context::{Cx, try_context},
    router::{Slot, href, layout},
    view::{Unescaped, View, class, view},
};

use crate::auth;

pub struct Bare;

pub const STYLE: &str = r#"
:root {
  color-scheme: light dark;
  --bg: #f6f6f4;
  --panel: #ffffff;
  --text: #1c1c1a;
  --muted: #6a6a64;
  --line: #deded8;
  --accent: #2b6cb0;
  --accent-text: #ffffff;
  --warn: #b7791f;
  --bad: #c53030;
  --track: #e6e6e0;
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg: #161615;
    --panel: #1f1f1d;
    --text: #e8e8e4;
    --muted: #9a9a92;
    --line: #34342f;
    --accent: #63a1e8;
    --accent-text: #0c1a2b;
    --warn: #e0b04a;
    --bad: #f08080;
    --track: #2e2e2a;
  }
}
* { box-sizing: border-box; }
html { scrollbar-gutter: stable; }
body {
  margin: 0;
  background: var(--bg);
  color: var(--text);
  font: 15px/1.4 system-ui, -apple-system, "Segoe UI", Roboto, sans-serif;
}
a { color: inherit; }
header.bar {
  border-bottom: 1px solid var(--line);
  background: var(--panel);
}
.frame {
  width: 100%;
  max-width: 1120px;
  margin: 0 auto;
  padding: 0 16px;
}
header.bar .frame {
  display: flex;
  align-items: center;
  gap: 24px;
  height: 52px;
}
.brand { font-weight: 700; letter-spacing: 0.02em; }
nav { display: flex; gap: 4px; flex: 1; min-width: 0; }
nav a {
  display: inline-flex;
  align-items: center;
  height: 32px;
  padding: 0 12px;
  border-radius: 6px;
  text-decoration: none;
  color: var(--muted);
}
nav a[aria-current="page"] { background: var(--track); color: var(--text); font-weight: 600; }
nav a:hover { color: var(--text); }
main.frame { padding-top: 20px; padding-bottom: 40px; }
h1 { font-size: 20px; margin: 0 0 16px; height: 28px; line-height: 28px; }
h2 {
  font-size: 12px;
  text-transform: uppercase;
  letter-spacing: 0.08em;
  color: var(--muted);
  margin: 0;
  height: 36px;
  display: flex;
  align-items: center;
  padding: 0 12px;
  border-bottom: 1px solid var(--line);
}
.notice {
  height: 24px;
  line-height: 24px;
  margin: 0 0 8px;
  color: var(--warn);
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.notice.error { color: var(--bad); }
.launcher { display: flex; gap: 8px; height: 40px; margin-bottom: 16px; }
.launcher input { flex: 1; min-width: 0; }
input[type="text"], input[type="password"], input[type="number"], textarea {
  height: 40px;
  padding: 0 12px;
  border: 1px solid var(--line);
  border-radius: 6px;
  background: var(--panel);
  color: var(--text);
  font: inherit;
}
button {
  height: 40px;
  padding: 0 16px;
  border: 1px solid var(--line);
  border-radius: 6px;
  background: var(--panel);
  color: var(--text);
  font: inherit;
  cursor: pointer;
}
button.primary { background: var(--accent); border-color: var(--accent); color: var(--accent-text); font-weight: 600; }
button.small { height: 28px; padding: 0 10px; font-size: 13px; }
.grid {
  display: grid;
  grid-template-columns: minmax(0, 3fr) minmax(0, 2fr);
  gap: 16px;
  align-items: start;
}
@media (max-width: 800px) { .grid { grid-template-columns: minmax(0, 1fr); } }
.card {
  background: var(--panel);
  border: 1px solid var(--line);
  border-radius: 8px;
  overflow: hidden;
}
.scroll { height: 440px; overflow-y: auto; }
.scroll.short { height: 200px; }
.row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  align-items: center;
  gap: 12px;
  height: 64px;
  padding: 0 12px;
  border-bottom: 1px solid var(--line);
}
.row .main { min-width: 0; }
.row .title, .row .meta {
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.row .title { font-weight: 600; }
.meta { color: var(--muted); font-size: 13px; }
.bar-track { height: 4px; border-radius: 2px; background: var(--track); margin-top: 4px; }
.bar-fill { height: 100%; border-radius: 2px; background: var(--accent); }
.empty { padding: 16px 12px; color: var(--muted); }
.log { padding: 8px 12px; font-size: 13px; color: var(--muted); }
.log div { height: 20px; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
.decision { padding: 12px; border-bottom: 1px solid var(--line); }
.decision h3 { margin: 0 0 4px; font-size: 15px; }
.decision .details { margin: 8px 0; font-size: 13px; color: var(--muted); max-height: 96px; overflow-y: auto; }
.choices { display: grid; gap: 6px; margin-top: 8px; }
.choices button { height: auto; min-height: 36px; text-align: left; padding: 6px 12px; }
.chapters { display: grid; gap: 6px; margin: 8px 0; }
.chapter {
  display: grid;
  grid-template-columns: 72px 72px minmax(0, 1fr);
  gap: 6px;
}
.chapter input { height: 32px; min-width: 0; }
.login {
  max-width: 360px;
  margin: 18vh auto 0;
  padding: 24px;
}
.login form { display: grid; gap: 12px; }
.login .notice { height: 24px; }
.placeholder { color: var(--muted); }
form.inline { display: inline; margin: 0; }
"#;

#[layout("/")]
pub async fn shell(cx: &Cx, slot: Slot<'_>) -> Result<impl View> {
    let bare = try_context::<Bare>(cx).is_some();
    let signed_in = auth::auth(cx).required();
    let pages: [(&'static str, bool, String); 4] = [
        (
            "Activity",
            href!(crate::activity::page).is_current(cx),
            href!(crate::activity::page).resolve(cx),
        ),
        (
            "Watchlist",
            href!(crate::pages::watchlist).is_current(cx),
            href!(crate::pages::watchlist).resolve(cx),
        ),
        (
            "Library",
            href!(crate::pages::library).is_current(cx),
            href!(crate::pages::library).resolve(cx),
        ),
        (
            "Settings",
            href!(crate::pages::settings).is_current(cx),
            href!(crate::pages::settings).resolve(cx),
        ),
    ];
    Ok(view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>"muzik"</title>
                <style>(Unescaped::new_unchecked(STYLE))</style>
                if !bare {
                    topcoat::runtime::script()
                }
            </head>
            <body>
                if !bare {
                    <header class="bar">
                        <div class="frame">
                            <span class="brand">"muzik"</span>
                            <nav>
                                for (label, current, target) in pages {
                                    <a
                                        href=(target)
                                        aria-current=(current.then_some("page"))
                                        class=(class!("nav-link", "current" if current))
                                    >(label)</a>
                                }
                            </nav>
                            if signed_in {
                                <form class="inline" method="post" action=(href!(crate::pages::logout))>
                                    <button class="small" type="submit">"Sign out"</button>
                                </form>
                            }
                        </div>
                    </header>
                    <main class="frame">(slot)</main>
                } else {
                    (slot)
                }
            </body>
        </html>
    })
}
