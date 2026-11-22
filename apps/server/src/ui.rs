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
form.inline { display: inline-flex; align-items: center; gap: 6px; margin: 0; }
a.button {
  display: inline-flex;
  align-items: center;
  height: 28px;
  padding: 0 10px;
  border: 1px solid var(--line);
  border-radius: 6px;
  background: var(--panel);
  color: var(--text);
  font-size: 13px;
  text-decoration: none;
}
button.danger { color: var(--bad); }
button:disabled { opacity: 0.5; cursor: default; }
.watch {
  display: grid;
  grid-template-columns: 260px minmax(0, 1fr);
  gap: 16px;
  align-items: start;
}
@media (max-width: 800px) { .watch { grid-template-columns: minmax(0, 1fr); } }
.scroll.rail { height: 700px; }
.source {
  display: grid;
  align-content: center;
  height: 52px;
  padding: 0 12px;
  border-bottom: 1px solid var(--line);
  text-decoration: none;
  min-width: 0;
}
.source.current { background: var(--track); }
.source .title, .source .meta {
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.source .title { font-weight: 600; }
.src-head { height: 56px; padding: 8px 12px 0; border-bottom: 1px solid var(--line); }
.src-head .title { font-weight: 600; font-size: 16px; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
.tools {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
  min-height: 48px;
  padding: 10px 12px;
  border-bottom: 1px solid var(--line);
}
.tools .renamer input { height: 28px; width: 180px; font-size: 13px; }
.card > .notice { padding: 0 12px; margin: 4px 0 0; }
.tabs {
  display: flex;
  gap: 4px;
  height: 40px;
  align-items: center;
  padding: 0 12px;
  border-bottom: 1px solid var(--line);
  overflow-x: auto;
}
.tab {
  display: inline-flex;
  align-items: center;
  height: 28px;
  padding: 0 10px;
  border-radius: 6px;
  text-decoration: none;
  color: var(--muted);
  font-size: 13px;
  white-space: nowrap;
}
.tab.current { background: var(--track); color: var(--text); font-weight: 600; }
.scroll.items { height: 352px; }
.scroll.card-detail { height: 232px; border-top: 1px solid var(--line); }
.irow {
  display: grid;
  grid-template-columns: 40px minmax(0, 1fr) 72px 104px 72px;
  align-items: center;
  gap: 8px;
  height: 44px;
  padding: 0 12px;
  border-bottom: 1px solid var(--line);
}
.irow.current { background: var(--track); }
.irow .num { color: var(--muted); font-size: 13px; }
.irow .main { min-width: 0; }
.irow .title, .irow .meta {
  display: block;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  line-height: 18px;
}
.irow .meta { font-size: 12px; height: 16px; }
.irow .act { display: flex; justify-content: flex-end; }
.pips { display: inline-flex; gap: 3px; }
.pip { display: inline-block; width: 12px; height: 4px; border-radius: 2px; background: var(--track); border: 1px solid transparent; }
.pip.done { background: #2f855a; }
.pip.run { background: var(--accent); }
.pip.wait { background: var(--warn); opacity: 0.5; }
.pip.fail { background: var(--bad); }
.pip.stale { background: var(--warn); }
.pip.skip { background: transparent; border-color: var(--muted); }
.tag {
  display: inline-block;
  max-width: 100%;
  padding: 1px 8px;
  border-radius: 10px;
  border: 1px solid var(--line);
  font-size: 12px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.tag.info { color: var(--accent); border-color: var(--accent); }
.tag.warn { color: var(--warn); border-color: var(--warn); }
.tag.bad { color: var(--bad); border-color: var(--bad); }
.tag.ok { color: #2f855a; border-color: #2f855a; }
.tag.mute { color: var(--muted); }
.ic { display: grid; grid-template-columns: 160px minmax(0, 1fr) 200px; gap: 12px; padding: 12px; }
@media (max-width: 800px) { .ic { grid-template-columns: minmax(0, 1fr); } }
.ic-thumb { width: 160px; height: 90px; border-radius: 6px; background: var(--track); overflow: hidden; }
.ic-thumb img { width: 100%; height: 100%; object-fit: cover; display: block; }
.ic-body { min-width: 0; display: grid; gap: 4px; align-content: start; }
.ic-body .title { font-weight: 600; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
.meta.error { color: var(--bad); }
.stages { display: flex; flex-wrap: wrap; gap: 4px 12px; font-size: 13px; }
.stage { display: inline-flex; align-items: center; gap: 4px; }
.ic-actions { display: grid; gap: 6px; align-content: start; }
.ic-actions form { display: block; }
.ic-actions button { width: 100%; }
.summary { height: 60px; margin-bottom: 8px; }
.summary .meta {
  height: 20px;
  line-height: 20px;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.lib {
  display: grid;
  grid-template-columns: minmax(0, 3fr) minmax(0, 2fr);
  gap: 16px;
  align-items: start;
}
@media (max-width: 800px) { .lib { grid-template-columns: minmax(0, 1fr); } }
.lrow {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1.2fr) 48px 52px 60px 64px 88px;
  align-items: center;
  gap: 8px;
  height: 40px;
  padding: 0 12px;
  border-bottom: 1px solid var(--line);
}
.lrow.current { background: var(--track); }
.lrow.lhead {
  height: 36px;
  font-size: 12px;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: var(--muted);
}
.lhead a { text-decoration: none; overflow: hidden; white-space: nowrap; }
.lrow .cell, .tcell .title, .tcell .meta {
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.lrow .num, .trow .num { text-align: right; }
.scroll.libtable { height: 440px; }
.scroll.libdetail { height: 480px; }
.pager {
  display: flex;
  align-items: center;
  gap: 8px;
  height: 44px;
  padding: 0 12px;
  border-top: 1px solid var(--line);
}
.pager .meta { flex: 1; }
.facts {
  display: grid;
  grid-template-columns: 80px minmax(0, 1fr);
  gap: 4px 12px;
  margin: 0;
  padding: 12px;
  border-bottom: 1px solid var(--line);
}
.facts dt { color: var(--muted); }
.facts dd { margin: 0; overflow-wrap: anywhere; }
.trow {
  display: grid;
  grid-template-columns: 40px minmax(0, 1fr) 56px;
  align-items: center;
  gap: 8px;
  height: 40px;
  padding: 0 12px;
  border-bottom: 1px solid var(--line);
}
.trow.thead { height: 32px; font-size: 12px; color: var(--muted); }
.tcell { display: grid; min-width: 0; }
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
            href!(crate::watchlist::page).is_current(cx),
            href!(crate::watchlist::page).resolve(cx),
        ),
        (
            "Library",
            href!(crate::library::page).is_current(cx),
            href!(crate::library::page).resolve(cx),
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
