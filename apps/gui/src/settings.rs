use super::*;
use crate::backend::SoulseekForm;
use gpui_kit::component::group_box::GroupBoxVariant;
use gpui_kit::component::input::{NumberInput, Textarea, TextareaState};
use gpui_kit::component::setting::{
    NumberFieldOptions, RenderOptions, SettingField, SettingGroup, SettingItem, SettingPage,
    Settings,
};
use muzik_soulseek::session::{DEFAULT_SERVER_HOST, DEFAULT_SERVER_PORT};

const PATHS: [(&str, &str, bool); 3] = [
    ("output", "Downloads", true),
    ("splits", "Splits", true),
    ("config", "Beets config", false),
];

const BANDCAMP_HELP: &str = "Muzik uses your Bandcamp login to read your collection and download your purchases in FLAC. To get the login cookie:
1. In your browser, log in to bandcamp.com.
2. Open the developer tools (Option-Command-I), then open Storage (Firefox, Zen) or Application (Chrome).
3. Select Cookies, then https://bandcamp.com.
4. Double-click the Value of the identity row and copy it.
5. Paste it below and select Save Bandcamp login. Muzik finds your user name.
A full Cookie header or a cookies.txt file also works. The cookie stays on this computer. Do not share it.";

pub struct ConfigView {
    main: WeakEntity<Muzik>,
    defaults: GuiDefaults,
    paths: Vec<Entity<InputState>>,
    soulseek: SoulseekFields,
    bandcamp: BandcampFields,
    status: Rc<RefCell<String>>,
}

struct BandcampFields {
    user: Entity<InputState>,
    cookies: Entity<TextareaState>,
    logged_in: bool,
}

struct SoulseekFields {
    username: Entity<InputState>,
    password: Entity<InputState>,
    host: Entity<InputState>,
    port: Entity<InputState>,
    has_password: bool,
}

#[derive(Clone, Copy)]
enum ChoiceField {
    AudioSource,
    Metadata,
    Prefer,
    Fallback,
    QualityPolicy,
    Duplicates,
}

impl ChoiceField {
    const ALL: [Self; 6] = [
        Self::AudioSource,
        Self::Metadata,
        Self::Prefer,
        Self::Fallback,
        Self::QualityPolicy,
        Self::Duplicates,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::AudioSource => "Audio source",
            Self::Metadata => "Metadata",
            Self::Prefer => "Prefer",
            Self::Fallback => "Fallback",
            Self::QualityPolicy => "Quality policy",
            Self::Duplicates => "Album already in library",
        }
    }

    const fn values(self) -> &'static [&'static str] {
        match self {
            Self::AudioSource => AudioSource::CHOICES,
            Self::Metadata => MetadataSource::CHOICES,
            Self::Prefer => muzik_core::PreferredAudio::CHOICES,
            Self::Fallback => AudioFallback::CHOICES,
            Self::QualityPolicy => QualityPolicy::CHOICES,
            Self::Duplicates => DuplicatePolicy::CHOICES,
        }
    }

    fn current(self, defaults: &GuiDefaults) -> String {
        match self {
            Self::AudioSource => defaults.audio_source.to_string(),
            Self::Metadata => defaults.metadata_source.to_string(),
            Self::Prefer => defaults.prefer.to_string(),
            Self::Fallback => defaults.fallback.to_string(),
            Self::QualityPolicy => defaults.quality_policy.to_string(),
            Self::Duplicates => defaults.duplicates.to_string(),
        }
    }

    fn set(self, defaults: &mut GuiDefaults, value: &str) {
        let _ = match self {
            Self::AudioSource => value
                .parse()
                .map(|value| defaults.audio_source = value)
                .ok(),
            Self::Metadata => value
                .parse()
                .map(|value| defaults.metadata_source = value)
                .ok(),
            Self::Prefer => value.parse().map(|value| defaults.prefer = value).ok(),
            Self::Fallback => value.parse().map(|value| defaults.fallback = value).ok(),
            Self::QualityPolicy => value
                .parse()
                .map(|value| defaults.quality_policy = value)
                .ok(),
            Self::Duplicates => value.parse().map(|value| defaults.duplicates = value).ok(),
        };
    }
}

#[derive(Clone, Copy)]
enum NumberField {
    Jobs,
    MinBitrate,
}

impl NumberField {
    const ALL: [Self; 2] = [Self::Jobs, Self::MinBitrate];

    const fn label(self) -> &'static str {
        match self {
            Self::Jobs => "Jobs",
            Self::MinBitrate => "Min bitrate",
        }
    }

    const fn step(self) -> f64 {
        match self {
            Self::Jobs => 1.,
            Self::MinBitrate => 32.,
        }
    }

    fn current(self, defaults: &GuiDefaults) -> f64 {
        match self {
            Self::Jobs => u64_to_f64(u64::try_from(defaults.jobs).unwrap_or(u64::MAX)),
            Self::MinBitrate => f64::from(defaults.min_bitrate),
        }
    }

    fn set(self, defaults: &mut GuiDefaults, value: f64) {
        match self {
            Self::Jobs => {
                defaults.jobs = usize::try_from(f64_to_u64(value)).unwrap_or(usize::MAX);
            }
            Self::MinBitrate => {
                defaults.min_bitrate = u32::try_from(f64_to_u64(value)).unwrap_or(u32::MAX);
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Flag {
    Review,
    NoSplit,
    NoOrganize,
    Import,
    TagOnly,
    DryRun,
    KeepSource,
    Force,
    Interactive,
}

impl Flag {
    const ALL: [Self; 9] = [
        Self::Review,
        Self::NoSplit,
        Self::NoOrganize,
        Self::Import,
        Self::TagOnly,
        Self::DryRun,
        Self::KeepSource,
        Self::Force,
        Self::Interactive,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Review => "Review chapters",
            Self::NoSplit => "No split",
            Self::NoOrganize => "No organize",
            Self::Import => "Import",
            Self::TagOnly => "Tag only",
            Self::DryRun => "Dry run",
            Self::KeepSource => "Keep source",
            Self::Force => "Force",
            Self::Interactive => "Interactive",
        }
    }

    const fn field(self, defaults: &mut GuiDefaults) -> &mut bool {
        match self {
            Self::Review => &mut defaults.review,
            Self::NoSplit => &mut defaults.no_split,
            Self::NoOrganize => &mut defaults.no_organize,
            Self::Import => &mut defaults.import,
            Self::TagOnly => &mut defaults.tag_only,
            Self::DryRun => &mut defaults.dry_run,
            Self::KeepSource => &mut defaults.keep_source,
            Self::Force => &mut defaults.force,
            Self::Interactive => &mut defaults.interactive,
        }
    }
}

fn edit(view: &WeakEntity<ConfigView>, cx: &mut App, change: impl FnOnce(&mut GuiDefaults)) {
    let _ = view.update(cx, |view, cx| {
        change(&mut view.defaults);
        cx.notify();
    });
}

fn control_width<E: Styled>(element: E, options: &RenderOptions, width: Pixels) -> E {
    if options.layout() == Axis::Horizontal {
        element.w(width)
    } else {
        element.w_full()
    }
}

fn input_item(label: &'static str, state: &Entity<InputState>) -> SettingItem {
    let state = state.clone();
    SettingItem::new(
        label,
        SettingField::render(move |options, _, _| {
            control_width(Input::new(&state).aria_label(label), options, px(256.))
        }),
    )
}

impl ConfigView {
    pub(crate) fn new(
        main: &Entity<Muzik>,
        defaults: GuiDefaults,
        status: Rc<RefCell<String>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(main, |_, _, cx| cx.notify()).detach();
        let paths = [&defaults.output, &defaults.splits, &defaults.config]
            .into_iter()
            .zip(PATHS)
            .map(|(value, (_, label, _))| {
                let value = value.display().to_string();
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(label)
                        .default_value(value)
                })
            })
            .collect();
        let soulseek = SoulseekFields {
            username: cx.new(|cx| InputState::new(window, cx).placeholder("Username")),
            password: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Password")
                    .masked(true)
            }),
            host: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(DEFAULT_SERVER_HOST)
                    .default_value(DEFAULT_SERVER_HOST)
            }),
            port: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(DEFAULT_SERVER_PORT.to_string())
                    .default_value(DEFAULT_SERVER_PORT.to_string())
                    .step(1.)
                    .min(1.)
                    .max(65535.)
            }),
            has_password: false,
        };
        let bandcamp = BandcampFields {
            user: cx.new(|cx| InputState::new(window, cx).placeholder("Found automatically")),
            cookies: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Paste the identity cookie value")
                    .rows(4)
            }),
            logged_in: false,
        };
        Self {
            main: main.downgrade(),
            defaults,
            paths,
            soulseek,
            bandcamp,
            status,
        }
    }

    pub(crate) fn set_bandcamp(
        &mut self,
        settings: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.bandcamp.logged_in = settings["logged_in"] == true;
        if let Some(user) = settings["user"].as_str().filter(|user| !user.is_empty()) {
            let user = user.to_string();
            self.bandcamp
                .user
                .update(cx, |state, cx| state.set_value(user, window, cx));
        }
        let placeholder = if self.bandcamp.logged_in {
            "Saved. Paste new cookies to change them."
        } else {
            "Paste the identity cookie value"
        };
        self.bandcamp.cookies.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.set_placeholder(placeholder, window, cx);
        });
        cx.notify();
    }

    fn save_bandcamp(&self, cx: &mut Context<Self>) {
        let user = self.bandcamp.user.read(cx).value().trim().to_string();
        let cookies = self.bandcamp.cookies.read(cx).value().to_string();
        if let Some(main) = self.main.upgrade() {
            main.update(cx, |main, cx| main.save_bandcamp(user, cookies, cx));
        }
    }

    fn logout_bandcamp(&self, cx: &mut Context<Self>) {
        if let Some(main) = self.main.upgrade() {
            main.update(cx, Muzik::logout_bandcamp);
        }
    }

    pub(crate) fn set_soulseek(
        &mut self,
        settings: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = |key: &str| describe(&settings[key]);
        for (state, value) in [
            (&self.soulseek.username, text("username")),
            (&self.soulseek.host, text("server_host")),
            (&self.soulseek.port, text("server_port")),
        ] {
            if !value.is_empty() {
                state.update(cx, |state, cx| state.set_value(value, window, cx));
            }
        }
        self.soulseek.has_password = settings["has_password"] == true;
        let placeholder = if self.soulseek.has_password {
            "Saved. Type a new one to change it."
        } else {
            "Password"
        };
        self.soulseek.password.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.set_placeholder(placeholder, window, cx);
        });
        cx.notify();
    }

    fn soulseek_form(&self, cx: &App) -> Option<SoulseekForm> {
        let username = self.soulseek.username.read(cx).value().trim().to_string();
        if username.is_empty() {
            return None;
        }
        Some(SoulseekForm {
            username,
            password: self.soulseek.password.read(cx).value().to_string(),
            server_host: self.soulseek.host.read(cx).value().trim().to_string(),
            server_port: self.soulseek.port.read(cx).value().trim().to_string(),
        })
    }

    fn pick_path(index: usize, window: &Window, cx: &Context<Self>) {
        let directories = PATHS.get(index).is_some_and(|(_, _, directory)| *directory);
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: !directories,
            directories,
            multiple: false,
            prompt: Some("Select".into()),
        });
        cx.spawn_in(window, async move |view, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await
                && let Some(path) = paths.into_iter().next()
            {
                let value = path.to_string_lossy().into_owned();
                let _ = view.update_in(cx, |view, window, cx| {
                    if let Some(state) = view.paths.get(index) {
                        state.update(cx, |state, cx| state.set_value(value, window, cx));
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn save(&self, cx: &mut Context<Self>) {
        let mut defaults = self.defaults.clone();
        for (state, path) in self.paths.iter().zip([
            &mut defaults.output,
            &mut defaults.splits,
            &mut defaults.config,
        ]) {
            *path = PathBuf::from(state.read(cx).value().to_string());
        }
        let soulseek = self.soulseek_form(cx);
        if let Some(main) = self.main.upgrade() {
            main.update(cx, |main, cx| main.save_config(defaults, soulseek, cx));
        }
        *self.status.borrow_mut() = "Saving config".into();
        cx.notify();
    }

    fn workflow_page(&self, cx: &Context<Self>) -> SettingPage {
        let view = cx.entity().downgrade();
        SettingPage::new("Workflow")
            .description("Workflow uses these settings for each run.")
            .resettable(false)
            .group(self.destinations_group(&view))
            .group(self.quality_group(&view))
            .group(
                SettingGroup::new()
                    .title("Processing")
                    .items(self.number_items(&view))
                    .items(self.switch_items(&view)),
            )
            .group(self.agent_group(&view))
    }

    fn destinations_group(&self, view: &WeakEntity<Self>) -> SettingGroup {
        SettingGroup::new().title("Destinations").items(
            self.paths
                .iter()
                .zip(PATHS)
                .enumerate()
                .map(|(index, (state, (_, label, _)))| {
                    let state = state.clone();
                    let view = view.clone();
                    SettingItem::new(
                        label,
                        SettingField::render(move |_, _, _| {
                            let view = view.clone();
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .w_full()
                                .child(div().flex_1().child(Input::new(&state).aria_label(label)))
                                .child(
                                    Button::new(("config-pick", index))
                                        .icon(IconName::FolderOpen)
                                        .label("Choose…")
                                        .on_click(move |_, window, cx| {
                                            let _ = view.update(cx, |_, cx| {
                                                Self::pick_path(index, window, cx);
                                            });
                                        }),
                                )
                        }),
                    )
                    .layout(Axis::Vertical)
                }),
        )
    }

    fn quality_group(&self, view: &WeakEntity<Self>) -> SettingGroup {
        SettingGroup::new()
            .title("Sources and quality")
            .items(ChoiceField::ALL.into_iter().map(|field| {
                let view = view.clone();
                let current = field.current(&self.defaults);
                SettingItem::new(
                    field.label(),
                    SettingField::dropdown(
                        field
                            .values()
                            .iter()
                            .map(|value| (SharedString::from(*value), SharedString::from(*value)))
                            .collect(),
                        move |_| current.clone().into(),
                        move |value, cx| {
                            edit(&view, cx, |defaults| field.set(defaults, &value));
                        },
                    ),
                )
            }))
    }

    fn number_items(&self, view: &WeakEntity<Self>) -> Vec<SettingItem> {
        NumberField::ALL
            .into_iter()
            .map(|field| {
                let view = view.clone();
                let current = field.current(&self.defaults);
                SettingItem::new(
                    field.label(),
                    SettingField::number_input(
                        NumberFieldOptions {
                            min: 0.,
                            step: field.step(),
                            ..NumberFieldOptions::default()
                        },
                        move |_| current,
                        move |value, cx| {
                            edit(&view, cx, |defaults| field.set(defaults, value));
                        },
                    ),
                )
            })
            .collect()
    }

    fn switch_items(&self, view: &WeakEntity<Self>) -> Vec<SettingItem> {
        let mut current = self.defaults.clone();
        Flag::ALL
            .into_iter()
            .map(|flag| {
                let enabled = *flag.field(&mut current);
                let view = view.clone();
                SettingItem::new(
                    flag.label(),
                    SettingField::switch(
                        move |_| enabled,
                        move |checked, cx| {
                            edit(&view, cx, |defaults| *flag.field(defaults) = checked);
                        },
                    ),
                )
            })
            .collect()
    }

    fn agent_group(&self, view: &WeakEntity<Self>) -> SettingGroup {
        let model = self.defaults.agent_model.clone();
        let auto_decide = self.defaults.auto_decide;
        SettingGroup::new()
            .title("AI decisions")
            .description("Muzik asks this Codex model to pick album matches and Soulseek downloads. It asks you when the model is not sure.")
            .item(SettingItem::new(
                "Model",
                SettingField::input(move |_| model.clone().into(), {
                    let view = view.clone();
                    move |value, cx| edit(&view, cx, |defaults| defaults.agent_model = value.into())
                }),
            ))
            .item(SettingItem::new(
                "Choose automatically",
                SettingField::switch(move |_| auto_decide, {
                    let view = view.clone();
                    move |checked, cx| edit(&view, cx, |defaults| defaults.auto_decide = checked)
                }),
            ))
    }

    fn accounts_page(&self, cx: &Context<Self>) -> SettingPage {
        let view = cx.entity().downgrade();
        let port = self.soulseek.port.clone();
        let soulseek = SettingGroup::new()
            .title("Soulseek")
            .description(
                "Your Soulseek account. Workflow uses it when Audio source is soulseek or as a fallback.",
            )
            .item(input_item("Username", &self.soulseek.username))
            .item(input_item("Password", &self.soulseek.password))
            .item(input_item("Server", &self.soulseek.host))
            .item(SettingItem::new(
                "Port",
                SettingField::render(move |options, _, _| {
                    control_width(
                        NumberInput::new(&port).accessibility_label("Port"),
                        options,
                        px(140.),
                    )
                }),
            ));
        let cookies = self.bandcamp.cookies.clone();
        let logged_in = self.bandcamp.logged_in;
        let bandcamp = SettingGroup::new()
            .title("Bandcamp")
            .item(SettingItem::render(|_, _, cx| {
                style::meta(BANDCAMP_HELP, cx)
            }))
            .item(input_item("User name", &self.bandcamp.user))
            .item(
                SettingItem::new(
                    "Login cookie",
                    SettingField::render(move |_, _, _| {
                        Textarea::new(&cookies)
                            .aria_label("Login cookie")
                            .w_full()
                            .h(px(96.))
                    }),
                )
                .layout(Axis::Vertical),
            )
            .item(SettingItem::render(move |_, _, cx| {
                let save = view.clone();
                let logout = view.clone();
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        Button::new("save-bandcamp")
                            .label("Save Bandcamp login")
                            .on_click(move |_, _, cx| {
                                let _ = save.update(cx, |view, cx| view.save_bandcamp(cx));
                            }),
                    )
                    .when(logged_in, |row| {
                        row.child(
                            Button::new("logout-bandcamp")
                                .ghost()
                                .label("Log out")
                                .on_click(move |_, _, cx| {
                                    let _ = logout.update(cx, |view, cx| view.logout_bandcamp(cx));
                                }),
                        )
                        .child(style::meta("Logged in", cx))
                    })
            }));
        SettingPage::new("Accounts")
            .description("The accounts that muzik uses to find and download audio.")
            .resettable(false)
            .group(soulseek)
            .group(bandcamp)
    }

    fn services_page(&self) -> SettingPage {
        let main = self.main.clone();
        SettingPage::new("Services")
            .description("The tools muzik can use.")
            .resettable(false)
            .group(
                SettingGroup::new()
                    .title("Services")
                    .item(SettingItem::render(move |_, _, cx| {
                        Muzik::services_section(main.clone(), cx)
                    })),
            )
    }
}

impl Render for ConfigView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.status.borrow().clone();
        let pages = [
            self.workflow_page(cx),
            self.accounts_page(cx),
            self.services_page(),
        ];
        div()
            .v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(
                div().flex_1().min_h_0().child(
                    Settings::new("settings")
                        .with_group_variant(GroupBoxVariant::Outline)
                        .sidebar_width(px(200.))
                        .pages(pages),
                ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .p_4()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().background)
                    .child(div().text_sm().child(status))
                    .child(
                        Button::new("save-config")
                            .primary()
                            .label("Save config")
                            .on_click(cx.listener(|view, _, _, cx| view.save(cx))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{ChoiceField, GuiDefaults};

    #[test]
    fn every_dropdown_value_becomes_the_current_choice() {
        let mut defaults = GuiDefaults::default();
        for field in ChoiceField::ALL {
            for value in field.values() {
                field.set(&mut defaults, value);
                assert_eq!(field.current(&defaults), *value, "{}", field.label());
            }
        }
    }
}
