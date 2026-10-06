use super::*;
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

pub(crate) struct ConfigView {
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

fn choices(defaults: &GuiDefaults) -> [(&'static str, &'static [&'static str], String); 6] {
    [
        (
            "Audio source",
            AudioSource::CHOICES,
            defaults.audio_source.to_string(),
        ),
        (
            "Metadata",
            MetadataSource::CHOICES,
            defaults.metadata_source.to_string(),
        ),
        (
            "Prefer",
            muzik_core::PreferredAudio::CHOICES,
            defaults.prefer.to_string(),
        ),
        (
            "Fallback",
            AudioFallback::CHOICES,
            defaults.fallback.to_string(),
        ),
        (
            "Quality policy",
            QualityPolicy::CHOICES,
            defaults.quality_policy.to_string(),
        ),
        (
            "Album already in library",
            DuplicatePolicy::CHOICES,
            defaults.duplicates.to_string(),
        ),
    ]
}

fn set_choice(defaults: &mut GuiDefaults, index: usize, value: &str) {
    let _ = match index {
        0 => value
            .parse()
            .ok()
            .map(|value| defaults.audio_source = value),
        1 => value
            .parse()
            .ok()
            .map(|value| defaults.metadata_source = value),
        2 => value.parse().ok().map(|value| defaults.prefer = value),
        3 => value.parse().ok().map(|value| defaults.fallback = value),
        4 => value
            .parse()
            .ok()
            .map(|value| defaults.quality_policy = value),
        5 => value.parse().ok().map(|value| defaults.duplicates = value),
        _ => None,
    };
}

fn flags(defaults: &mut GuiDefaults) -> [(&'static str, &mut bool); 9] {
    [
        ("Review chapters", &mut defaults.review),
        ("No split", &mut defaults.no_split),
        ("No organize", &mut defaults.no_organize),
        ("Import", &mut defaults.import),
        ("Tag only", &mut defaults.tag_only),
        ("Dry run", &mut defaults.dry_run),
        ("Keep source", &mut defaults.keep_source),
        ("Force", &mut defaults.force),
        ("Interactive", &mut defaults.interactive),
    ]
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
            control_width(Input::new(&state), options, px(256.))
        }),
    )
}

impl ConfigView {
    pub(crate) fn new(
        main: Entity<Muzik>,
        defaults: GuiDefaults,
        status: Rc<RefCell<String>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&main, |_, _, cx| cx.notify()).detach();
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

    fn send_bandcamp(&mut self, command: &str, cx: &mut Context<Self>) {
        let params = json!({
            "user": self.bandcamp.user.read(cx).value().trim().to_string(),
            "cookies": self.bandcamp.cookies.read(cx).value().to_string(),
        });
        if let Some(main) = self.main.upgrade() {
            main.update(cx, |main, cx| {
                main.error = None;
                main.send(command, params);
                cx.notify();
            });
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

    fn soulseek_params(&self, cx: &App) -> Option<Value> {
        let username = self.soulseek.username.read(cx).value().trim().to_string();
        if username.is_empty() {
            return None;
        }
        Some(json!({
            "username": username,
            "password": self.soulseek.password.read(cx).value().to_string(),
            "server_host": self.soulseek.host.read(cx).value().trim().to_string(),
            "server_port": self.soulseek.port.read(cx).value().trim().to_string(),
        }))
    }

    fn pick_path(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let directories = PATHS.get(index).is_some_and(|(_, _, directory)| *directory);
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: !directories,
            directories,
            multiple: false,
            prompt: Some("Select".into()),
        });
        cx.spawn_in(window, async move |view, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                if let Some(path) = paths.into_iter().next() {
                    let value = path.to_string_lossy().into_owned();
                    let _ = view.update_in(cx, |view, window, cx| {
                        if let Some(state) = view.paths.get(index) {
                            state.update(cx, |state, cx| state.set_value(value, window, cx));
                        }
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let mut defaults = self.defaults.clone();
        for (state, path) in self.paths.iter().zip([
            &mut defaults.output,
            &mut defaults.splits,
            &mut defaults.config,
        ]) {
            *path = PathBuf::from(state.read(cx).value().to_string());
        }
        let params = match serde_json::to_value(&defaults) {
            Ok(params) => params,
            Err(error) => {
                *self.status.borrow_mut() = error.to_string();
                cx.notify();
                return;
            }
        };
        let soulseek = self.soulseek_params(cx);
        if let Some(main) = self.main.upgrade() {
            main.update(cx, |main, cx| {
                main.error = None;
                main.send("config.save", params);
                if let Some(soulseek) = soulseek {
                    main.send("soulseek.save", soulseek);
                }
                cx.notify();
            });
        }
        *self.status.borrow_mut() = "Saving config".into();
        cx.notify();
    }

    fn workflow_page(&self, cx: &mut Context<Self>) -> SettingPage {
        let view = cx.entity().downgrade();
        let destinations = SettingGroup::new().title("Destinations").items(
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
                                .child(div().flex_1().child(Input::new(&state)))
                                .child(
                                    Button::new(("config-pick", index))
                                        .icon(IconName::FolderOpen)
                                        .label("Choose…")
                                        .on_click(move |_, window, cx| {
                                            let _ = view.update(cx, |view, cx| {
                                                view.pick_path(index, window, cx)
                                            });
                                        }),
                                )
                        }),
                    )
                    .layout(Axis::Vertical)
                }),
        );
        let quality = SettingGroup::new().title("Sources and quality").items(
            choices(&self.defaults).into_iter().enumerate().map(
                |(index, (label, values, current))| {
                    let view = view.clone();
                    SettingItem::new(
                        label,
                        SettingField::dropdown(
                            values
                                .iter()
                                .map(|value| {
                                    (SharedString::from(*value), SharedString::from(*value))
                                })
                                .collect(),
                            move |_| current.clone().into(),
                            move |value, cx| {
                                edit(&view, cx, |defaults| set_choice(defaults, index, &value))
                            },
                        ),
                    )
                },
            ),
        );
        let numbers = [
            ("Jobs", self.defaults.jobs as f64, 1.),
            ("Min bitrate", f64::from(self.defaults.min_bitrate), 32.),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (label, current, step))| {
            let view = view.clone();
            SettingItem::new(
                label,
                SettingField::number_input(
                    NumberFieldOptions {
                        min: 0.,
                        step,
                        ..NumberFieldOptions::default()
                    },
                    move |_| current,
                    move |value, cx| {
                        edit(&view, cx, |defaults| match index {
                            0 => defaults.jobs = value as usize,
                            _ => defaults.min_bitrate = value as u32,
                        })
                    },
                ),
            )
        });
        let mut current = self.defaults.clone();
        let switches = flags(&mut current)
            .into_iter()
            .enumerate()
            .map(|(index, (label, enabled))| {
                let enabled = *enabled;
                let view = view.clone();
                SettingItem::new(
                    label,
                    SettingField::switch(
                        move |_| enabled,
                        move |checked, cx| {
                            edit(&view, cx, |defaults| {
                                if let Some((_, flag)) = flags(defaults).into_iter().nth(index) {
                                    *flag = checked;
                                }
                            })
                        },
                    ),
                )
            })
            .collect::<Vec<_>>();
        let model = self.defaults.agent_model.clone();
        let auto_decide = self.defaults.auto_decide;
        let agent = SettingGroup::new()
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
            ));
        SettingPage::new("Workflow")
            .description("Workflow uses these settings for each run.")
            .resettable(false)
            .group(destinations)
            .group(quality)
            .group(
                SettingGroup::new()
                    .title("Processing")
                    .items(numbers)
                    .items(switches),
            )
            .group(agent)
    }

    fn accounts_page(&self, cx: &mut Context<Self>) -> SettingPage {
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
                    control_width(NumberInput::new(&port), options, px(140.))
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
                        Textarea::new(&cookies).w_full().h(px(96.))
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
                                let _ = save
                                    .update(cx, |view, cx| view.send_bandcamp("bandcamp.save", cx));
                            }),
                    )
                    .when(logged_in, |row| {
                        row.child(
                            Button::new("logout-bandcamp")
                                .ghost()
                                .label("Log out")
                                .on_click(move |_, _, cx| {
                                    let _ = logout.update(cx, |view, cx| {
                                        view.send_bandcamp("bandcamp.logout", cx)
                                    });
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
