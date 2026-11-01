use crate::model::list;
use crate::{
    bridge::Bridge,
    locale::{Localizer, language},
    model::*,
};
use gpui::{prelude::*, *};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

const ACCENT: u32 = 0xb8ed45;
const ROOMS: [(&str, &str); 6] = [
    ("overview", "Now"),
    ("processes", "Apps"),
    ("workloads", "Workloads"),
    ("health", "Battery Health"),
    ("insights", "Insights"),
    ("settings", "Settings"),
];
#[derive(Clone)]
enum Action {
    Room(usize),
    Sort(&'static str),
    Range(&'static str),
    Post(String, Value),
    Copy(String),
    Refresh,
}
#[derive(Clone, Copy)]
struct Theme {
    bg: u32,
    card: u32,
    ink: u32,
    muted: u32,
    line: u32,
}
fn theme(window: &Window) -> Theme {
    match window.appearance() {
        WindowAppearance::Dark | WindowAppearance::VibrantDark => Theme {
            bg: 0x171d19,
            card: 0x222b24,
            ink: 0xe9efe5,
            muted: 0xa5b29e,
            line: 0x354132,
        },
        _ => Theme {
            bg: 0xf2f4ee,
            card: 0xffffff,
            ink: 0x1b281c,
            muted: 0x697363,
            line: 0xe0e6d9,
        },
    }
}

pub struct BatteryHog {
    pub focus: FocusHandle,
    locale: Localizer,
    bridge: Option<Bridge>,
    stats: Value,
    history: Value,
    room: usize,
    sort: &'static str,
    range: &'static str,
    history_request: u64,
    pending: HashSet<u64>,
    last_sample: Option<Instant>,
    last_history: Instant,
    power: Vec<(f64, Option<f32>)>,
    message: String,
    disconnected: bool,
}
impl BatteryHog {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let bridge = Bridge::start();
        let message = bridge.as_ref().err().cloned().unwrap_or_default();
        let mut view = Self {
            focus: cx.focus_handle(),
            locale: Localizer::new(),
            bridge: bridge.ok(),
            stats: Value::Null,
            history: Value::Null,
            room: 0,
            sort: "score",
            range: "24h",
            history_request: 0,
            pending: HashSet::new(),
            last_sample: None,
            last_history: Instant::now(),
            power: vec![],
            message,
            disconnected: false,
        };
        if let Ok(language) = std::env::var("BATTERY_HOG_LANGUAGE") {
            view.locale.language = crate::locale::language(&[language]).into();
        }
        cx.spawn(async move |view, cx| {
            loop {
                smol::Timer::after(Duration::from_millis(120)).await;
                if view
                    .update(cx, |view, cx| {
                        let events: Vec<_> = view
                            .bridge
                            .as_ref()
                            .map(|b| b.events.try_iter().collect())
                            .unwrap_or_default();
                        let changed = !events.is_empty();
                        for event in events {
                            view.receive(event, cx);
                        }
                        if view.room == 3
                            && view.last_history.elapsed() > Duration::from_secs(10)
                            && (view.history["refreshing"] == true
                                || view.history["loading"] == true
                                || view.history.is_null())
                        {
                            view.load_history();
                        }
                        if changed {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        view
    }
    fn t(&self, key: &str) -> String {
        self.locale.text(key)
    }
    fn f(&self, key: &str, values: &[(&str, String)]) -> String {
        self.locale.format(key, values)
    }
    fn receive(&mut self, event: Value, cx: &mut Context<Self>) {
        match event["event"].as_str().unwrap_or("") {
            "ready" => {
                if std::env::var("BATTERY_HOG_LANGUAGE").is_err() {
                    self.locale.language = language(
                        &list(&event["languages"])
                            .iter()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect::<Vec<_>>(),
                    )
                    .into();
                }
                cx.set_menus(vec![Menu {
                    name: "Battery Hog".into(),
                    items: vec![MenuItem::action(self.t("Quit Battery Hog"), crate::Quit)],
                }]);
            }
            "stats" => {
                self.stats = event["body"].clone();
                self.last_sample = Some(Instant::now());
                self.disconnected = false;
                let timestamp = n(&self.stats["ts"]);
                if self.power.last().map(|s| s.0) != Some(timestamp) {
                    self.power.push((
                        timestamp,
                        num(&self.stats["power"]["watts"]).map(|v| v as f32),
                    ));
                    self.power.retain(|(ts, _)| timestamp - *ts <= 90.);
                }
            }
            "open" => {
                self.room = ROOMS
                    .iter()
                    .position(|(room, _)| *room == event["room"].as_str().unwrap_or("overview"))
                    .unwrap_or(0);
                cx.activate(true);
                for window in cx.windows() {
                    let _ = window.update(cx, |_, window, _| window.activate_window());
                }
            }
            "quit" => cx.quit(),
            "disconnected" => {
                self.disconnected = true;
                self.pending.clear();
                self.message = self.t("Monitor disconnected. Press Refresh to reconnect.");
            }
            "response" => {
                let id = event["id"].as_u64().unwrap_or(0);
                let was_pending = self.pending.remove(&id);
                if event["status"].as_u64().unwrap_or(500) >= 400 || event["body"]["ok"] == false {
                    self.message = text(&event["body"]["message"]);
                } else if event["path"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with("/api/history")
                    && id == self.history_request
                {
                    self.history = event["body"].clone();
                } else if event["path"] == "/api/stats" {
                    self.stats = event["body"].clone();
                    self.last_sample = Some(Instant::now());
                } else if was_pending {
                    self.message = if event["body"]["cancelled"] == true {
                        String::new()
                    } else {
                        self.t("Saved.")
                    };
                    if event["body"]["settings"].is_object() {
                        self.stats["settings"] = event["body"]["settings"].clone();
                    }
                }
            }
            _ => {}
        }
    }
    fn request(&mut self, method: &str, path: &str, body: Value) -> u64 {
        if let Some(bridge) = &mut self.bridge {
            match bridge.request(method, path, body) {
                Ok(id) => {
                    if method == "POST" {
                        self.pending.insert(id);
                    }
                    return id;
                }
                Err(error) => self.message = error,
            }
        }
        0
    }
    fn load_history(&mut self) {
        self.last_history = Instant::now();
        self.history_request = self.request(
            "GET",
            &format!("/api/history?range={}", self.range),
            json!({}),
        );
    }
    fn act(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            Action::Room(room) => {
                self.room = room;
                window.focus(&self.focus);
                if room == 3 {
                    self.load_history();
                }
            }
            Action::Sort(sort) => self.sort = sort,
            Action::Range(range) => {
                self.range = range;
                self.history = Value::Null;
                self.load_history();
            }
            Action::Post(path, body) => {
                if self.pending.is_empty() {
                    self.message = self.t("Saving…");
                    self.request("POST", &path, body);
                }
            }
            Action::Copy(value) => {
                cx.write_to_clipboard(ClipboardItem::new_string(value));
                self.message = self.t("Gate command copied");
            }
            Action::Refresh => {
                if self.disconnected || self.bridge.is_none() {
                    self.bridge = None;
                    match Bridge::start() {
                        Ok(b) => {
                            self.bridge = Some(b);
                            self.message = String::new();
                            self.disconnected = false
                        }
                        Err(e) => self.message = e,
                    }
                }
                self.request("GET", "/api/stats", json!({}));
                if self.room == 3 {
                    self.load_history();
                }
            }
        }
        cx.notify();
    }
    fn button(
        &self,
        id: impl Into<String>,
        label: String,
        action: Action,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let busy = !self.pending.is_empty() && matches!(action, Action::Post(..));
        let keyboard_action = action.clone();
        div()
            .id(SharedString::from(id.into()))
            .tab_index(0)
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if !busy && (event.keystroke.key == "enter" || event.keystroke.key == "space") {
                    this.act(keyboard_action.clone(), window, cx);
                    cx.stop_propagation();
                }
            }))
            .px_3()
            .py_2()
            .rounded_lg()
            .bg(rgb(0xdef2b5))
            .text_color(rgb(0x273a12))
            .text_sm()
            .cursor_pointer()
            .hover(|s| s.bg(rgb(ACCENT)))
            .focus(|s| s.border_2().border_color(rgb(0x78a832)))
            .when(busy, |s| s.opacity(0.5))
            .child(label)
            .on_click(cx.listener(move |this, _, window, cx| {
                if !busy {
                    this.act(action.clone(), window, cx)
                }
            }))
    }
    fn card(&self, title: &str, t: Theme) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .p_5()
            .rounded_xl()
            .bg(rgb(t.card))
            .border_1()
            .border_color(rgb(t.line))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(t.muted))
                    .child(self.t(title)),
            )
    }
    fn row(&self, label: &str, value: String, t: Theme) -> Div {
        div()
            .flex()
            .justify_between()
            .items_center()
            .gap_4()
            .py_2()
            .border_b_1()
            .border_color(rgb(t.line))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(t.muted))
                    .child(self.t(label)),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(value),
            )
    }
    fn tile(&self, label: &str, value: String, detail: String, t: Theme) -> Div {
        self.card(label, t)
            .flex_1()
            .min_w(px(150.))
            .child(div().text_2xl().font_weight(FontWeight::BOLD).child(value))
            .child(div().text_sm().text_color(rgb(t.muted)).child(detail))
    }
    fn toggle(
        &self,
        id: &str,
        label: &str,
        detail: &str,
        on: bool,
        patch: Value,
        t: Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .py_3()
            .border_b_1()
            .border_color(rgb(t.line))
            .child(
                div().flex_1().child(self.t(label)).child(
                    div()
                        .text_sm()
                        .text_color(rgb(t.muted))
                        .child(self.t(detail)),
                ),
            )
            .child(self.button(
                id,
                self.t(if on { "On" } else { "Off" }),
                Action::Post("/api/settings".into(), patch),
                cx,
            ))
    }
    fn overview(&self, t: Theme, cx: &mut Context<Self>) -> Div {
        let b = &self.stats["battery"];
        let d = &self.stats["dev_summary"];
        let procs = sorted_processes(&self.stats, "score");
        let state = match b["state"].as_str().unwrap_or("") {
            "charging" => "Charging",
            "charged" => "Fully charged",
            "ac" => "Plugged in",
            _ => "On battery",
        };
        let pct = num(&b["percent"])
            .map(|n| format!("{n:.0}%"))
            .unwrap_or_else(|| self.t("No battery"));
        let hero = div()
            .flex()
            .flex_col()
            .gap_5()
            .p_6()
            .rounded_xl()
            .bg(rgb(0x17241c))
            .text_color(rgb(0xeaf3dc))
            .flex_1()
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(ACCENT))
                    .child(self.t("Energy forecast")),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .gap_4()
                    .child(
                        div().text_3xl().font_weight(FontWeight::BOLD).child(
                            b["time"]
                                .as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| self.t(state)),
                        ),
                    )
                    .child(div().text_3xl().child(pct.clone())),
            )
            .child(self.t(state))
            .child(metric(&self.stats["power"]["watts"], " W"))
            .child(
                div().h_2().rounded_full().bg(rgb(0x394b38)).child(
                    div()
                        .h_2()
                        .rounded_full()
                        .bg(rgb(ACCENT))
                        .w(relative((n(&b["percent"]) / 100.).clamp(0., 1.) as f32)),
                ),
            );
        let mut cause = self.card("What is driving it", t).flex_1();
        if n(&d["projects"]) > 0. {
            cause = cause
                .child(div().text_2xl().font_weight(FontWeight::BOLD).child(self.f(
                    "{count} dev projects pulling together",
                    &[("count", text(&d["projects"]))],
                )))
                .child(self.f(
                    "{count} heavy · {cpu}% CPU",
                    &[
                        ("count", text(&d["heavy_workers"])),
                        ("cpu", format!("{:.0}", n(&d["cpu"]))),
                    ],
                ))
                .child(self.button(
                    "overview-workloads",
                    self.t("Open Workloads"),
                    Action::Room(2),
                    cx,
                ));
        } else if let Some(top) = procs.first() {
            cause = cause
                .child(
                    div()
                        .text_2xl()
                        .font_weight(FontWeight::BOLD)
                        .child(text(&top["name"])),
                )
                .child(self.f(
                    "It is using {cpu}% CPU and {memory} of memory in the latest sample.",
                    &[
                        ("cpu", format!("{:.0}", n(&top["cpu"]))),
                        ("memory", mem(&top["mem_mb"])),
                    ],
                ))
                .child(self.button("overview-apps", self.t("Open Apps"), Action::Room(1), cx));
        } else {
            cause = cause.child(self.t("Your Mac is running quietly"));
        }
        let m = &self.stats["memory"];
        let pressure = self.t(match m["pressure"].as_str().unwrap_or("") {
            "high" => "High",
            "elevated" => "Elevated",
            _ => "Normal",
        });
        let trace = self
            .power
            .iter()
            .map(|(_, v)| v.map(|v| (v, false)))
            .collect();
        let end = self.power.last().map(|s| s.0).unwrap_or(0.);
        let positions = Some(
            self.power
                .iter()
                .map(|(ts, _)| ((ts - end + 90.) / 90.).clamp(0., 1.) as f32)
                .collect(),
        );
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(div().flex().gap_4().child(hero).child(cause))
            .child(
                self.card("Power current · 90 seconds", t)
                    .child(chart(trace, false, positions))
                    .child(
                        div().text_sm().text_color(rgb(t.muted)).child(
                            self.t("Live system-wide watts; samples arrive every 5 seconds."),
                        ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .child(self.tile(
                        "Memory pressure",
                        pressure,
                        format!(
                            "{} / {} GB",
                            metric(&m["used_gb"], ""),
                            metric(&m["total_gb"], "")
                        ),
                        t,
                    ))
                    .child(self.tile(
                        "Active apps",
                        procs.len().to_string(),
                        self.f("{count} cores", &[("count", text(&self.stats["ncpu"]))]),
                        t,
                    ))
                    .child(self.tile("Development", text(&d["projects"]), self.t("Projects"), t)),
            )
            .child(self.card("Low Power Mode", t).child(self.button(
                "lpm",
                self.t(if self.stats["lowpower"] == true {
                    "Turn off"
                } else {
                    "Turn on"
                }),
                Action::Post(
                    "/api/lowpower".into(),
                    json!({"on":self.stats["lowpower"]!=true}),
                ),
                cx,
            )))
            .child(self.observations(t, cx))
    }
    fn observations(&self, t: Theme, cx: &mut Context<Self>) -> Div {
        let mut card = self.card("Observations", t);
        let ignored = list(&self.stats["ignored"]);
        let hogs: Vec<_> = sorted_processes(&self.stats, "cpu")
            .into_iter()
            .filter(|p| {
                p["protected"] != true && !ignored.contains(&p["name"]) && n(&p["cpu"]) >= 40.
            })
            .take(3)
            .collect();
        if hogs.is_empty() {
            card = card.child(self.t("Nothing's hammering your battery right now."));
        }
        for p in hogs {
            let name = text(&p["name"]);
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div().flex_1().child(name.clone()).child(
                            div()
                                .text_sm()
                                .text_color(rgb(t.muted))
                                .child(metric(&p["cpu"], "% CPU")),
                        ),
                    )
                    .child(self.button(
                        format!("obs-quit-{name}"),
                        self.t("Quit"),
                        quit_action(&p),
                        cx,
                    ))
                    .child(self.button(
                        format!("mute-{name}"),
                        self.t("Mute"),
                        Action::Post("/api/ignore".into(), json!({"name":name,"on":true})),
                        cx,
                    )),
            );
        }
        if n(&self.stats["battery"]["percent"]) <= 20.
            && self.stats["battery"]["percent"].is_number()
        {
            card = card.child(self.t("Turn on Low Power Mode to stretch the remaining charge."));
        }
        if n(&self.stats["health"]["health"]) > 0. && n(&self.stats["health"]["health"]) < 80. {
            card = card.child(
                self.t("It holds less charge than when new — a service check may be worth it."),
            );
        }
        if !ignored.is_empty() {
            card = card.child(self.button(
                "reset-muted",
                self.t("Suggestions reset"),
                Action::Post("/api/ignore".into(), json!({"reset":true})),
                cx,
            ));
        }
        card
    }
    fn processes(&self, t: Theme, cx: &mut Context<Self>) -> Div {
        let mut page = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(t.muted))
                    .child(self.t("Relative CPU + memory estimate · not per-app watts")),
            )
            .child(
                div().flex().gap_2().children(
                    [("score", "Estimated"), ("cpu", "CPU"), ("mem_mb", "Memory")]
                        .into_iter()
                        .map(|(key, label)| {
                            self.button(
                                format!("sort-{key}"),
                                format!(
                                    "{}{}",
                                    if self.sort == key { "✓ " } else { "" },
                                    self.t(label)
                                ),
                                Action::Sort(key),
                                cx,
                            )
                        }),
                ),
            );
        let procs = sorted_processes(&self.stats, self.sort);
        if procs.is_empty() {
            page = page.child(self.t("No processes"));
        }
        for (i, p) in procs.into_iter().enumerate() {
            let name = text(&p["name"]);
            let icon = if let Some(path) = p["icon_path"].as_str() {
                img(std::path::PathBuf::from(path))
                    .w_8()
                    .h_8()
                    .into_any_element()
            } else {
                div()
                    .w_8()
                    .h_8()
                    .rounded_lg()
                    .bg(rgb(t.line))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(name.chars().take(1).collect::<String>())
                    .into_any_element()
            };
            let mut row = div()
                .flex()
                .items_center()
                .gap_4()
                .p_4()
                .rounded_lg()
                .bg(rgb(t.card))
                .border_1()
                .border_color(rgb(t.line))
                .child(
                    div()
                        .w_6()
                        .text_color(rgb(t.muted))
                        .child(format!("{:02}", i + 1)),
                )
                .child(icon)
                .child(
                    div().flex_1().min_w_0().child(name.clone()).child(
                        div()
                            .text_sm()
                            .text_color(rgb(t.muted))
                            .child(self.f("{count} processes", &[("count", text(&p["procs"]))])),
                    ),
                )
                .child(div().w(px(90.)).text_sm().child(self.t(
                    match p["level"].as_str().unwrap_or("") {
                        "high" => "High",
                        "med" => "Med",
                        _ => "Low",
                    },
                )))
                .child(div().w(px(85.)).text_sm().child(metric(&p["cpu"], "%")))
                .child(div().w(px(90.)).text_sm().child(mem(&p["mem_mb"])));
            if p["protected"] == true {
                row = row.child(div().w(px(65.)).text_sm().child(self.t("System")))
            } else {
                row = row.child(self.button(
                    format!("quit-{i}"),
                    self.t("Quit"),
                    quit_action(&p),
                    cx,
                ));
            }
            page = page.child(row);
        }
        page
    }
    fn workloads(&self, t: Theme, cx: &mut Context<Self>) -> Div {
        let dev = &self.stats["dev_summary"];
        let settings = &self.stats["settings"]["dev"];
        let mut page = div().flex().flex_col().gap_4().child(
            div()
                .flex()
                .gap_3()
                .child(self.tile(
                    "Projects",
                    text(&dev["projects"]),
                    self.t("Detected workloads"),
                    t,
                ))
                .child(self.tile(
                    "Dev workers",
                    text(&dev["workers"]),
                    self.t("Development"),
                    t,
                ))
                .child(self.tile(
                    "Combined CPU",
                    metric(&dev["cpu"], "%"),
                    self.t("Heavy now") + ": " + &text(&dev["heavy_workers"]),
                    t,
                )),
        );
        let mut work = self.card("Detected workloads", t);
        let workloads = list(&self.stats["workloads"]);
        if workloads.is_empty() {
            work = work.child(self.t("No development workloads detected right now."));
        }
        for w in workloads {
            let agents = list(&w["agents"])
                .iter()
                .map(text)
                .collect::<Vec<_>>()
                .join(" + ");
            work = work.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .py_3()
                    .border_b_1()
                    .border_color(rgb(t.line))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .child(text(&w["name"]))
                            .child(self.t(w["status"].as_str().unwrap_or("Active"))),
                    )
                    .child(div().text_sm().text_color(rgb(t.muted)).child(format!(
                        "{} · {}",
                        text(&w["path"]),
                        agents
                    )))
                    .child(format!(
                        "{} · {}",
                        metric(&w["cpu"], "% CPU"),
                        mem(&w["mem_mb"])
                    )),
            );
        }
        let mut controls = self.card("Battery-aware Agent Mode", t).child(self.toggle(
            "agent-mode",
            "Battery-aware Agent Mode",
            "Queue heavy commands on battery; stay unrestricted on AC.",
            settings["enabled"] == true,
            json!({"dev":{"enabled":settings["enabled"]!=true}}),
            t,
            cx,
        ));
        for (key, label, values) in [
            ("slots", "Heavy-job lanes", vec![1, 2, 3, 4]),
            ("workers", "Workers per job", vec![1, 2, 4, 8]),
            ("draw_threshold", "High-draw alert", vec![25, 30, 40]),
        ] {
            controls = controls.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(self.t(label))
                    .child(div().flex().gap_2().children(values.into_iter().map(|v| {
                        self.button(
                            format!("dev-{key}-{v}"),
                            format!(
                                "{}{}{}",
                                if n(&settings[key]) == v as f64 {
                                    "✓ "
                                } else {
                                    ""
                                },
                                v,
                                if key == "draw_threshold" { " W" } else { "" }
                            ),
                            Action::Post("/api/settings".into(), json!({"dev":{key:v}})),
                            cx,
                        )
                    }))),
            );
        }
        let command = self.stats["gate"]["command"].as_str().unwrap_or("");
        controls = controls
            .child(div().text_sm().child(command.to_string()))
            .child(self.button(
                "copy-gate",
                self.t("Copy"),
                Action::Copy(format!("{command} <command>")),
                cx,
            ));
        for key in ["active", "queued"] {
            for item in list(&self.stats["gate"][key]) {
                controls = controls.child(self.row(
                    if key == "active" { "Running" } else { "Queued" },
                    text(&item["label"]),
                    t,
                ));
            }
        }
        page = page.child(work).child(controls);
        page
    }
    fn health(&self, t: Theme, cx: &mut Context<Self>) -> Div {
        let h = &self.stats["health"];
        let b = &self.stats["battery"];
        let points = history_points(&self.history);
        let has = points.iter().any(Option::is_some);
        let mut history = self.card("Charge history", t).child(
            div().flex().gap_2().children(
                [("24h", "Last 24 Hours"), ("10d", "Last 10 Days")]
                    .into_iter()
                    .map(|(range, label)| {
                        self.button(
                            format!("range-{range}"),
                            format!(
                                "{}{}",
                                if range == self.range { "✓ " } else { "" },
                                self.t(label)
                            ),
                            Action::Range(range),
                            cx,
                        )
                    }),
            ),
        );
        history = history.child(if has {
            chart(points, true, None).into_any_element()
        } else {
            div()
                .h(px(180.))
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(t.muted))
                .child(self.t(
                    if self.history["loading"] == true || self.history.is_null() {
                        "Preparing charge history…"
                    } else {
                        "No charge history yet"
                    },
                ))
                .into_any_element()
        });
        let tick = |key: &str| {
            self.history[key]
                .as_i64()
                .and_then(|ts| chrono::TimeZone::timestamp_opt(&chrono::Local, ts, 0).single())
                .map(|d| {
                    d.format(if self.range == "24h" {
                        "%m/%d %H:%M"
                    } else {
                        "%m/%d"
                    })
                    .to_string()
                })
                .unwrap_or_else(|| "—".into())
        };
        history = history
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_sm()
                    .text_color(rgb(t.muted))
                    .child(tick("start"))
                    .child(tick("end")),
            )
            .child(self.row("Now", metric(&b["percent"], "%"), t));
        let bins = list(&self.history["bins"]);
        let lowest = bins
            .iter()
            .filter_map(|b| num(&b["lo"]).or_else(|| num(&b["v"])))
            .reduce(f64::min);
        let charges = bins
            .iter()
            .filter(|b| !b.is_null())
            .fold((0, false), |(count, previous), b| {
                let ac = b["ac"] == true;
                (count + usize::from(ac && !previous), ac)
            })
            .0;
        history = history
            .child(
                self.row(
                    "Lowest",
                    lowest
                        .map(|n| format!("{n:.0}%"))
                        .unwrap_or_else(|| "—".into()),
                    t,
                ),
            )
            .child(self.row(
                "Charges",
                if has {
                    charges.to_string()
                } else {
                    "—".into()
                },
                t,
            ));
        if self.history["refreshing"] == true {
            history = history.child(self.t("Refreshing"));
        }
        let details = self
            .card("Details", t)
            .child(self.row(
                "Condition",
                self.t(h["condition"].as_str().unwrap_or("Unknown")),
                t,
            ))
            .child(self.row("Cycle count", text(&h["cycles"]), t))
            .child(self.row("Temperature", metric(&h["temp_c"], " °C"), t))
            .child(self.row("Full charge capacity", metric(&h["full_mah"], " mAh"), t))
            .child(self.row("Design capacity", metric(&h["design_mah"], " mAh"), t));
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(self.tile(
                        "Maximum capacity",
                        metric(&h["health"], "%"),
                        self.t("of original capacity"),
                        t,
                    ))
                    .child(self.tile(
                        "Power source",
                        self.t(if b["on_ac"] == true {
                            "Power adapter"
                        } else {
                            "Battery"
                        }),
                        metric(&self.stats["power"]["watts"], " W"),
                        t,
                    )),
            )
            .child(history)
            .child(details)
    }
    fn insights(&self, t: Theme) -> Div {
        let insights = &self.stats["insights"];
        let today = &insights["today"];
        let week = &insights["week"];
        let runtime = num(&today["runtime_h"]).or_else(|| num(&week["runtime_h"]));
        let rate = num(&today["rate"]).or_else(|| num(&week["rate"]));
        let mut blockers = self.card("Sleep blockers", t);
        let items: Vec<_> = list(&self.stats["sleep_blockers"])
            .into_iter()
            .filter(|b| b["system"] != true && n(&b["duration_s"]) >= 60.)
            .collect();
        if items.is_empty() {
            blockers = blockers.child(self.t("No user apps are holding an idle-sleep assertion."));
        }
        for b in items {
            blockers = blockers
                .child(self.row(&text(&b["name"]), self.duration(n(&b["duration_s"])), t))
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(t.muted))
                        .child(text(&b["detail"])),
                );
        }
        if n(&self.stats["power_policy"]["display_sleep"]) >= 20. {
            blockers = blockers.child(self.f(
                "Display timeout is {minutes} minutes",
                &[(
                    "minutes",
                    text(&self.stats["power_policy"]["display_sleep"]),
                )],
            ));
        }
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                self.tile(
                    "Battery life on a charge",
                    runtime
                        .map(|v| self.f("{count}h", &[("count", format!("{v:.1}"))]))
                        .unwrap_or_else(|| "—".into()),
                    self.t(if runtime.is_none() {
                        "Not enough data yet"
                    } else {
                        "Battery life on a charge"
                    }),
                    t,
                ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .child(
                        self.tile(
                            "Drain rate",
                            rate.map(|r| self.f("{rate}%/h", &[("rate", format!("{r:.1}"))]))
                                .unwrap_or_else(|| "—".into()),
                            self.t("On battery"),
                            t,
                        ),
                    )
                    .child(self.tile(
                        "On battery today",
                        self.duration(n(&today["on_battery_h"]) * 3600.),
                        String::new(),
                        t,
                    ))
                    .child(self.tile(
                        "Charges today",
                        text(&insights["charges"]),
                        String::new(),
                        t,
                    ))
                    .child(self.tile(
                        "Overnight wake-ups",
                        text(&insights["wakes"]),
                        String::new(),
                        t,
                    )),
            )
            .child(blockers)
    }
    fn duration(&self, seconds: f64) -> String {
        if seconds >= 3600. {
            self.f(
                "{hours}h {minutes}m",
                &[
                    ("hours", format!("{:.0}", (seconds / 3600.).floor())),
                    ("minutes", format!("{:.0}", (seconds % 3600. / 60.).floor())),
                ],
            )
        } else {
            self.f(
                "{count}m",
                &[("count", format!("{:.0}", (seconds / 60.).floor()))],
            )
        }
    }
    fn settings(&self, t: Theme, cx: &mut Context<Self>) -> Div {
        let settings = &self.stats["settings"];
        let mb = &settings["menubar"];
        let mut menu = self.card("Menu bar", t);
        for (key, label, detail) in [
            ("percent", "Battery percentage", "e.g. 100%"),
            (
                "watts",
                "Power draw",
                "live wattage when on battery, e.g. 10W",
            ),
            ("time", "Time remaining", "e.g. 5:35"),
            ("hog", "Top app", "highest estimated impact + its CPU"),
            ("dev", "Development load", "active projects + heavy jobs"),
        ] {
            let on = if key == "percent" {
                mb[key] != false
            } else {
                mb[key] == true
            };
            menu = menu.child(self.toggle(
                &format!("mb-{key}"),
                label,
                detail,
                on,
                json!({"menubar":{key:!on}}),
                t,
                cx,
            ));
        }
        let mut alerts=self.card("Alerts",t)
            .child(self.toggle("alerts","Battery & usage alerts","Low battery, full charge, CPU spikes and sustained high draw",settings["alerts"]==true,json!({"alerts":settings["alerts"]!=true}),t,cx))
            .child(self.toggle("heat","Heat alerts","notify when thermal pressure rises, with the likely process or development workload causing it",settings["heat"]["enabled"]==true,json!({"heat":{"enabled":settings["heat"]["enabled"]!=true}}),t,cx));
        alerts = alerts.child(self.t("Low-battery alert at")).child(
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .children((1..=10).map(|i| i * 5).map(|v| {
                    self.button(
                        format!("threshold-{v}"),
                        format!(
                            "{}{}%",
                            if n(&settings["low_threshold"]) == v as f64 {
                                "✓ "
                            } else {
                                ""
                            },
                            v
                        ),
                        Action::Post("/api/settings".into(), json!({"low_threshold":v})),
                        cx,
                    )
                })),
        );
        div().flex().flex_col().gap_4().child(menu).child(alerts).child(self.card("Software updates",t).child(self.t("Updates disabled for this fork")).child(self.t("Build new versions from B416-JAFLY/BatteryHog. Upstream updates would replace the translations.")))
    }
}
fn quit_action(p: &Value) -> Action {
    Action::Post(
        "/api/kill".into(),
        json!({"name":p["name"],"path":p["path"],"bundle":p["bundle"],"pids":p["pids"]}),
    )
}

fn chart(
    points: Vec<Option<(f32, bool)>>,
    percent: bool,
    positions: Option<Vec<f32>>,
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let width: f32 = bounds.size.width.into();
            let height: f32 = bounds.size.height.into();
            let max = if percent {
                100.
            } else {
                points.iter().flatten().map(|(v, _)| *v).fold(10., f32::max) * 1.12
            };
            let pos = |i: usize, v: f32| {
                point(
                    bounds.origin.x
                        + px(positions
                            .as_ref()
                            .map(|p| p[i])
                            .unwrap_or((i as f32 + 0.5) / points.len().max(1) as f32)
                            * (width - 8.)
                            + 4.),
                    bounds.origin.y + px(height - 8. - v / max * (height - 16.)),
                )
            };
            for level in [0., 0.5, 1.] {
                let mut line = PathBuilder::stroke(px(1.));
                let y = bounds.origin.y + px(8. + (height - 16.) * level);
                line.move_to(point(bounds.origin.x, y));
                line.line_to(point(bounds.origin.x + px(width), y));
                if let Ok(path) = line.build() {
                    window.paint_path(path, rgba(0xa0b28e33));
                }
            }
            let mut line = PathBuilder::stroke(px(2.5));
            let mut active = false;
            for (i, point_value) in points.iter().enumerate() {
                if let Some((v, charging)) = point_value {
                    let p = pos(i, *v);
                    if active {
                        line.line_to(p)
                    } else {
                        line.move_to(p);
                        active = true
                    }
                    if *charging {
                        let mut band =
                            PathBuilder::stroke(px((width / points.len().max(1) as f32).max(2.)));
                        band.move_to(point(p.x, bounds.origin.y + px(height - 4.)));
                        band.line_to(point(p.x, bounds.origin.y + px(height - 9.)));
                        if let Ok(path) = band.build() {
                            window.paint_path(path, rgb(0x5d9c34));
                        }
                    }
                } else {
                    active = false
                }
            }
            if let Ok(path) = line.build() {
                window.paint_path(path, rgb(0x7aac32));
            }
        },
    )
    .w_full()
    .h(px(180.))
}
impl Render for BatteryHog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(window);
        let mut nav = div()
            .w(px(205.))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .bg(rgb(0x19261c))
            .text_color(rgb(0xe2eed6))
            .child(
                div()
                    .mt_3()
                    .mb_6()
                    .text_xl()
                    .font_weight(FontWeight::BOLD)
                    .child("Battery Hog"),
            );
        for (i, (_, label)) in ROOMS.iter().enumerate() {
            nav = nav.child(
                div()
                    .id(("nav", i))
                    .tab_index(0)
                    .on_key_down(cx.listener(move |view, event: &KeyDownEvent, window, cx| {
                        if event.keystroke.key == "enter" || event.keystroke.key == "space" {
                            view.act(Action::Room(i), window, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .px_4()
                    .py_3()
                    .rounded_lg()
                    .cursor_pointer()
                    .when(self.room == i, |s| {
                        s.bg(rgb(ACCENT)).text_color(rgb(0x203212))
                    })
                    .hover(|s| s.bg(rgb(0x3b4f2b)))
                    .child(self.t(label))
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.act(Action::Room(i), window, cx)
                    })),
            );
        }
        nav =
            nav.child(div().flex_1())
                .child(div().text_sm().text_color(rgb(0xb7c9a8)).child(self.t(
                    if self.disconnected {
                        "connection lost"
                    } else if self.last_sample.is_some() {
                        "Live"
                    } else {
                        "connecting…"
                    },
                )))
                .child(self.button("refresh", self.t("Refresh"), Action::Refresh, cx));
        let content = if self.stats.is_null() {
            div()
                .p_6()
                .text_xl()
                .child(self.t("Measuring your Mac"))
                .into_any_element()
        } else {
            match self.room {
                1 => self.processes(t, cx),
                2 => self.workloads(t, cx),
                3 => self.health(t, cx),
                4 => self.insights(t),
                5 => self.settings(t, cx),
                _ => self.overview(t, cx),
            }
            .into_any_element()
        };
        let mut body = div()
            .id("page-scroll")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .p_6()
            .flex()
            .flex_col()
            .gap_5()
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .child(self.t(ROOMS[self.room].1)),
            );
        if !self.message.is_empty() {
            body = body.child(
                div()
                    .p_3()
                    .rounded_lg()
                    .bg(rgb(0xe2edcf))
                    .text_color(rgb(0x344b1a))
                    .text_sm()
                    .child(self.message.clone()),
            );
        }
        div()
            .id("root")
            .track_focus(&self.focus)
            .tab_stop(false)
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, window, cx| {
                let key = &event.keystroke.key;
                if key == "tab" {
                    if event.keystroke.modifiers.shift {
                        window.focus_prev()
                    } else {
                        window.focus_next()
                    };
                    cx.stop_propagation();
                }
                if event.keystroke.modifiers.platform {
                    if key == "r" {
                        view.act(Action::Refresh, window, cx)
                    }
                    if let Ok(n) = key.parse::<usize>() {
                        if (1..=6).contains(&n) {
                            view.act(Action::Room(n - 1), window, cx)
                        }
                    }
                }
            }))
            .size_full()
            .flex()
            .font_family(".AppleSystemUIFont")
            .bg(rgb(t.bg))
            .text_color(rgb(t.ink))
            .child(nav)
            .child(body.child(content))
    }
}
