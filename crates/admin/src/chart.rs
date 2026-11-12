//! Charts: values over a period, drawn as a line or as bars.
//!
//! A screen reads a [`ChartQuery`] from its request, turns it into a
//! [`Period`], loads its numbers for that period, and renders a [`Chart`]:
//!
//! ```
//! use laterite_admin::chart::{Chart, ChartQuery, Kind, Period, Series};
//! use chrono::NaiveDate;
//!
//! let today = NaiveDate::from_ymd_opt(2026, 3, 31).unwrap();
//! let query = ChartQuery::default();
//! let period = Period::from_query(&query, today);
//! let rows = vec![(today, 12.0)];
//! let chart = Chart::new("calls", period.clone())
//!     .kind(Kind::Line)
//!     .series(Series::daily("calls", "Calls", &period, rows));
//! # let _ = chart;
//! ```
//!
//! The page lists [`CHART_ASSETS`] in its [`PageHead`](crate::PageHead).

use std::collections::BTreeMap;

use chrono::{Datelike, Duration, Months, NaiveDate};
use laterite_core::Text;
use serde::Deserialize;

use crate::form::escape_attr;
use crate::html::Markup;
use crate::Shell;

/// The assets a page with a chart or a period bar lists in its page head.
pub const CHART_ASSETS: &[&str] = &["chart.css"];

/// The longest custom period, in days.
const MAX_DAYS: i64 = 1830;
/// How many colours the palette cycles through.
const COLOURS: usize = 6;
/// The plot's width in drawing units; its height is 100.
const WIDTH: f64 = 1000.0;

/// The query parameters a period bar and a chart's legend write: `range`,
/// `from`, `to`, `by`, and one `show_<chart id>` per chart with hidden series.
/// A screen takes it with `Query<ChartQuery>`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ChartQuery {
    range: Option<String>,
    from: Option<String>,
    to: Option<String>,
    by: Option<String>,
    #[serde(flatten)]
    rest: BTreeMap<String, String>,
}

impl ChartQuery {
    /// The `show_<id>` parameters, as a query-string tail starting with `&`,
    /// leaving out the chart named `except`.
    fn shows(&self, except: &str) -> String {
        let skip = format!("show_{except}");
        self.rest
            .iter()
            .filter(|(k, _)| k.starts_with("show_") && **k != skip)
            .map(|(k, v)| format!("&{}={}", encode(k), encode(v)))
            .collect()
    }
}

/// How a period is cut into points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interval {
    /// One point a day.
    Day,
    /// One point a week, weeks starting on Monday.
    Week,
    /// One point a calendar month.
    Month,
}

impl Interval {
    fn code(self) -> &'static str {
        match self {
            Interval::Day => "day",
            Interval::Week => "week",
            Interval::Month => "month",
        }
    }

    fn start(self, date: NaiveDate) -> NaiveDate {
        match self {
            Interval::Day => date,
            Interval::Week => {
                date - Duration::days(i64::from(date.weekday().num_days_from_monday()))
            }
            Interval::Month => date.with_day(1).unwrap_or(date),
        }
    }

    fn next(self, start: NaiveDate) -> NaiveDate {
        match self {
            Interval::Day => start + Duration::days(1),
            Interval::Week => start + Duration::days(7),
            Interval::Month => start
                .checked_add_months(Months::new(1))
                .unwrap_or(start + Duration::days(31)),
        }
    }
}

/// The preset a period came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Range {
    Days(u32),
    ThisMonth,
    LastMonth,
    Year,
    Custom,
}

const PRESETS: &[(Range, &str, &str)] = &[
    (Range::Days(7), "7d", "7 days"),
    (Range::Days(30), "30d", "30 days"),
    (Range::Days(90), "90d", "90 days"),
    (Range::ThisMonth, "month", "This month"),
    (Range::LastMonth, "last_month", "Last month"),
    (Range::Year, "12m", "12 months"),
];

/// A span of days, both ends included, cut into points by an [`Interval`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Period {
    from: NaiveDate,
    to: NaiveDate,
    interval: Interval,
    range: Range,
    /// Set when the query named the interval.
    by: bool,
}

impl Period {
    /// The period a query asks for, counted back from `today`: the last 30
    /// days when it asks for nothing, or for something unreadable.
    pub fn from_query(query: &ChartQuery, today: NaiveDate) -> Self {
        let first = |d: NaiveDate| d.with_day(1).unwrap_or(d);
        let (range, from, to) = match query.range.as_deref() {
            Some("7d") => (Range::Days(7), today - Duration::days(6), today),
            Some("90d") => (Range::Days(90), today - Duration::days(89), today),
            Some("month") => (Range::ThisMonth, first(today), today),
            Some("last_month") => {
                let end = first(today) - Duration::days(1);
                (Range::LastMonth, first(end), end)
            }
            Some("12m") => {
                let start = first(today)
                    .checked_sub_months(Months::new(11))
                    .unwrap_or(today);
                (Range::Year, start, today)
            }
            Some("custom") => {
                let read = |s: &Option<String>| {
                    s.as_deref()
                        .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
                };
                match (read(&query.from), read(&query.to)) {
                    (Some(a), Some(b)) => {
                        let (a, b) = if a <= b { (a, b) } else { (b, a) };
                        let a = a.max(b - Duration::days(MAX_DAYS - 1));
                        (Range::Custom, a, b)
                    }
                    _ => (Range::Days(30), today - Duration::days(29), today),
                }
            }
            _ => (Range::Days(30), today - Duration::days(29), today),
        };
        let days = (to - from).num_days() + 1;
        let asked = match query.by.as_deref() {
            Some("day") if days <= 370 => Some(Interval::Day),
            Some("week") => Some(Interval::Week),
            Some("month") => Some(Interval::Month),
            _ => None,
        };
        let interval = asked.unwrap_or(match (range, days) {
            (Range::Year, _) => Interval::Month,
            (_, ..=92) => Interval::Day,
            (_, ..=370) => Interval::Week,
            _ => Interval::Month,
        });
        Self {
            from,
            to,
            interval,
            range,
            by: asked.is_some(),
        }
    }

    /// The first day.
    pub fn from(&self) -> NaiveDate {
        self.from
    }

    /// The last day.
    pub fn to(&self) -> NaiveDate {
        self.to
    }

    /// How the period is cut into points.
    pub fn interval(&self) -> Interval {
        self.interval
    }

    /// The first day of each point, in order.
    pub fn points(&self) -> Vec<NaiveDate> {
        let mut out = Vec::new();
        let mut at = self.interval.start(self.from);
        while at <= self.to {
            out.push(at);
            at = self.interval.next(at);
        }
        out
    }

    /// The period of the same length that ends the day before this one starts.
    pub fn previous(&self) -> Self {
        let days = (self.to - self.from).num_days() + 1;
        Self {
            from: self.from - Duration::days(days),
            to: self.from - Duration::days(1),
            interval: self.interval,
            range: Range::Custom,
            by: self.by,
        }
    }

    /// The period as query parameters, for a link that keeps it.
    pub fn query(&self) -> String {
        let mut out = match self.range {
            Range::Custom => format!("range=custom&from={}&to={}", self.from, self.to),
            range => {
                let code = PRESETS
                    .iter()
                    .find(|(r, ..)| *r == range)
                    .map_or("30d", |(_, code, _)| code);
                format!("range={code}")
            }
        };
        if self.by {
            out.push_str("&by=");
            out.push_str(self.interval.code());
        }
        out
    }

    /// Renders the period bar: the presets, and a from and to date with Apply.
    /// It keeps the series each chart in `query` shows.
    pub fn bar(&self, shell: &Shell, query: &ChartQuery) -> Markup {
        let shows = query.shows("");
        let mut presets = String::new();
        for (range, code, label) in PRESETS {
            let on = if *range == self.range { " is-on" } else { "" };
            presets.push_str(&format!(
                r#"<a class="lat-period__preset{on}" href="?range={code}{}">{}</a>"#,
                escape_attr(&shows),
                escape_attr(&shell.t(label)),
            ));
        }
        let hidden: String = query
            .rest
            .iter()
            .filter(|(k, _)| k.starts_with("show_"))
            .map(|(k, v)| {
                format!(
                    r#"<input type="hidden" name="{}" value="{}">"#,
                    escape_attr(k),
                    escape_attr(v)
                )
            })
            .collect();
        Markup::from_override(format!(
            r#"<form class="lat-period" method="get"><div class="lat-period__presets">{presets}</div><div class="lat-period__custom"><input type="hidden" name="range" value="custom">{hidden}<input class="lat-input" type="date" name="from" value="{from}" aria-label="{from_label}"><span>{to_word}</span><input class="lat-input" type="date" name="to" value="{to}" aria-label="{to_label}"><button class="lat-btn lat-btn--sm" type="submit">{apply}</button></div></form>"#,
            from = self.from,
            to = self.to,
            from_label = escape_attr(&shell.t("From")),
            to_label = escape_attr(&shell.t("To")),
            to_word = escape_attr(&shell.t("to")),
            apply = escape_attr(&shell.t("Apply")),
        ))
    }

    fn label(&self, point: NaiveDate) -> String {
        match self.interval {
            Interval::Month => point.format("%b %Y").to_string(),
            _ => point.format("%-d %b").to_string(),
        }
    }
}

/// One line, or one set of bars: a value for each point of the period.
#[derive(Clone, Debug)]
pub struct Series {
    key: String,
    label: Text,
    values: Vec<f64>,
}

impl Series {
    /// A series from one value per point of the period, in order. Missing
    /// points count as zero and extra ones are dropped when the chart is drawn.
    pub fn new(key: impl Into<String>, label: impl Into<Text>, values: Vec<f64>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            values,
        }
    }

    /// A series from dated rows: each row is added to the point its day falls
    /// in, a day outside the period is left out, and a point with no row is zero.
    pub fn daily(
        key: impl Into<String>,
        label: impl Into<Text>,
        period: &Period,
        rows: impl IntoIterator<Item = (NaiveDate, f64)>,
    ) -> Self {
        let points = period.points();
        let mut values = vec![0.0; points.len()];
        for (day, value) in rows {
            if day < period.from || day > period.to {
                continue;
            }
            let start = period.interval.start(day);
            if let Ok(i) = points.binary_search(&start) {
                values[i] += value;
            }
        }
        Self::new(key, label, values)
    }

    /// The sum of the series over the period.
    pub fn total(&self) -> f64 {
        self.values.iter().sum()
    }
}

/// How a chart draws its series.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A line per series.
    Line,
    /// Bars side by side.
    Bar,
    /// Bars stacked into one per point.
    StackedBar,
}

/// How a chart writes a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Format {
    /// A number: whole when it is whole, else two decimals.
    Number,
    /// An amount with two decimals after a currency code.
    Money(String),
}

impl Format {
    fn write(&self, value: f64) -> String {
        match self {
            Format::Number if value.fract() == 0.0 => grouped(value, 0),
            Format::Number => grouped(value, 2),
            Format::Money(code) => format!("{code} {}", grouped(value, 2)),
        }
    }
}

/// A chart: a title, a legend that shows and hides its series, and the plot.
#[derive(Clone, Debug)]
pub struct Chart {
    id: String,
    period: Period,
    kind: Kind,
    format: Format,
    title: Option<Text>,
    series: Vec<Series>,
}

impl Chart {
    /// A line chart over `period`. `id` names it on the page: letters, digits,
    /// `-` and `_`.
    pub fn new(id: impl Into<String>, period: Period) -> Self {
        Self {
            id: id.into(),
            period,
            kind: Kind::Line,
            format: Format::Number,
            title: None,
            series: Vec::new(),
        }
    }

    /// Sets how the series are drawn.
    pub fn kind(mut self, kind: Kind) -> Self {
        self.kind = kind;
        self
    }

    /// Sets how values are written.
    pub fn format(mut self, format: Format) -> Self {
        self.format = format;
        self
    }

    /// Sets the title above the legend.
    pub fn title(mut self, title: impl Into<Text>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Adds a series.
    pub fn series(mut self, series: Series) -> Self {
        self.series.push(series);
        self
    }

    /// Renders the chart, showing the series `query` asks for (all of them
    /// when it names none).
    pub fn render(&self, shell: &Shell, query: &ChartQuery) -> Markup {
        let points = self.period.points();
        let n = points.len().max(1);
        let id = escape_attr(&self.id);
        let param = format!("show_{}", self.id);
        let asked: Vec<&str> = query
            .rest
            .get(&param)
            .map(|s| s.split(',').collect())
            .unwrap_or_default();
        let mut shown: Vec<bool> = self
            .series
            .iter()
            .map(|s| asked.contains(&s.key.as_str()))
            .collect();
        if !shown.contains(&true) {
            shown = vec![true; self.series.len()];
        }
        let value = |s: &Series, i: usize| s.values.get(i).copied().unwrap_or(0.0).max(0.0);
        let visible: Vec<(usize, &Series)> = self
            .series
            .iter()
            .enumerate()
            .filter(|(i, _)| shown[*i])
            .collect();

        // The scale: the tallest point, rounded up to a clean number.
        let tallest = (0..n)
            .map(|i| {
                let at = visible.iter().map(|(_, s)| value(s, i));
                if self.kind == Kind::StackedBar {
                    at.sum::<f64>()
                } else {
                    at.fold(0.0, f64::max)
                }
            })
            .fold(0.0, f64::max);
        let top = clean_top(tallest);
        let y = |v: f64| 100.0 - v / top * 100.0;
        let step = WIDTH / n as f64;

        // The legend: a chip per series that hides or shows it.
        let base = format!("?{}{}", self.period.query(), query.shows(&self.id));
        let link = |keys: Vec<&str>| {
            let href = if keys.len() == self.series.len() || keys.is_empty() {
                base.clone()
            } else {
                format!("{base}&{}={}", encode(&param), encode(&keys.join(",")))
            };
            let href = escape_attr(&href);
            format!(
                r##"href="{href}" hx-get="{href}" hx-target="#{id}" hx-select="#{id}" hx-swap="outerHTML" hx-push-url="true""##
            )
        };
        let mut legend = String::new();
        for (i, s) in self.series.iter().enumerate() {
            let after: Vec<&str> = self
                .series
                .iter()
                .enumerate()
                .filter(|(j, _)| if *j == i { !shown[*j] } else { shown[*j] })
                .map(|(_, s)| s.key.as_str())
                .collect();
            // The last series left stays on.
            let after = if after.is_empty() {
                vec![s.key.as_str()]
            } else {
                after
            };
            legend.push_str(&format!(
                r#"<a class="lat-chart__chip{off}" {link} aria-pressed="{pressed}" role="button"><i style="background:var(--chart-{c})"></i><span>{label}</span><b>{total}</b></a>"#,
                off = if shown[i] { "" } else { " is-off" },
                link = link(after),
                pressed = shown[i],
                c = i % COLOURS + 1,
                label = escape_attr(&shell.tt(&s.label)),
                total = escape_attr(&self.format.write(s.total())),
            ));
        }
        if shown.contains(&false) {
            legend.push_str(&format!(
                r#"<a class="lat-chart__all" {}>{}</a>"#,
                link(Vec::new()),
                escape_attr(&shell.t("Show all")),
            ));
        }

        // The plot.
        let mut svg = String::new();
        for line in [25, 50, 75] {
            svg.push_str(&format!(
                r#"<line class="lat-chart__grid" x1="0" x2="{WIDTH}" y1="{line}" y2="{line}"/>"#
            ));
        }
        match self.kind {
            Kind::Line => {
                for (c, s) in &visible {
                    let path: Vec<String> = (0..n)
                        .map(|i| format!("{:.1} {:.2}", (i as f64 + 0.5) * step, y(value(s, i))))
                        .collect();
                    let colour = c % COLOURS + 1;
                    if visible.len() == 1 {
                        svg.push_str(&format!(
                            r#"<path class="lat-chart__fill" style="fill:var(--chart-{colour})" d="M{} L{:.1} 100 L{:.1} 100 Z"/>"#,
                            path.join(" L"),
                            (n as f64 - 0.5) * step,
                            0.5 * step,
                        ));
                    }
                    svg.push_str(&format!(
                        r#"<path class="lat-chart__line" style="stroke:var(--chart-{colour})" d="M{}"/>"#,
                        path.join(" L"),
                    ));
                }
            }
            Kind::Bar | Kind::StackedBar => {
                let stacked = self.kind == Kind::StackedBar;
                let inner = step * 0.7;
                let width = if stacked {
                    inner
                } else {
                    inner / visible.len().max(1) as f64
                };
                for i in 0..n {
                    let left = i as f64 * step + step * 0.15;
                    let mut floor = 0.0;
                    for (k, (c, s)) in visible.iter().enumerate() {
                        let v = value(s, i);
                        if v <= 0.0 {
                            continue;
                        }
                        let (x, bottom) = if stacked {
                            (left, floor)
                        } else {
                            (left + width * k as f64, 0.0)
                        };
                        floor += if stacked { v } else { 0.0 };
                        svg.push_str(&format!(
                            r#"<rect style="fill:var(--chart-{})" x="{x:.1}" y="{:.2}" width="{width:.1}" height="{:.2}"/>"#,
                            c % COLOURS + 1,
                            y(bottom + v),
                            v / top * 100.0,
                        ));
                    }
                }
            }
        }

        // One column per point: the value card and, on a line, the markers.
        let mut columns = String::new();
        let mut table = String::new();
        for (i, point) in points.iter().enumerate() {
            let label = escape_attr(&self.period.label(*point));
            let mut tip = format!("<strong>{label}</strong>");
            let mut dots = String::new();
            let mut cells = String::new();
            for (c, s) in &visible {
                let v = value(s, i);
                let written = escape_attr(&self.format.write(v));
                let colour = c % COLOURS + 1;
                tip.push_str(&format!(
                    r#"<span><i style="background:var(--chart-{colour})"></i>{}<b>{written}</b></span>"#,
                    escape_attr(&shell.tt(&s.label)),
                ));
                if self.kind == Kind::Line {
                    dots.push_str(&format!(
                        r#"<i class="lat-chart__dot" style="bottom:{:.2}%;background:var(--chart-{colour})"></i>"#,
                        v / top * 100.0,
                    ));
                }
                cells.push_str(&format!("<td>{written}</td>"));
            }
            let side = if i * 2 >= n { " is-right" } else { "" };
            columns.push_str(&format!(
                r#"<div class="lat-chart__col{side}">{dots}<div class="lat-chart__tip">{tip}</div></div>"#
            ));
            table.push_str(&format!(r#"<tr><th scope="row">{label}</th>{cells}</tr>"#));
        }
        let heads: String = visible
            .iter()
            .map(|(_, s)| {
                format!(
                    r#"<th scope="col">{}</th>"#,
                    escape_attr(&shell.tt(&s.label))
                )
            })
            .collect();

        // The axes: five values up the side, a handful of dates along the bottom.
        let ticks: String = [1.0, 0.75, 0.5, 0.25, 0.0]
            .iter()
            .map(|f| format!("<span>{}</span>", escape_attr(&short(top * f))))
            .collect();
        let every = n.div_ceil(6);
        let dates: String = points
            .iter()
            .enumerate()
            .filter(|(i, _)| i % every == 0)
            .map(|(i, p)| {
                format!(
                    r#"<span style="grid-column:{}">{}</span>"#,
                    i + 1,
                    escape_attr(&self.period.label(*p))
                )
            })
            .collect();
        let title = self.title.as_ref().map_or(String::new(), |t| {
            format!(
                r#"<figcaption class="lat-chart__title">{}</figcaption>"#,
                escape_attr(&shell.tt(t))
            )
        });
        let empty = if tallest <= 0.0 {
            format!(
                r#"<p class="lat-chart__empty">{}</p>"#,
                escape_attr(&shell.t("Nothing in this period."))
            )
        } else {
            String::new()
        };
        Markup::from_override(format!(
            r#"<figure class="lat-chart" id="{id}">{title}<div class="lat-chart__legend">{legend}</div><div class="lat-chart__body"><div class="lat-chart__y" aria-hidden="true">{ticks}</div><div class="lat-chart__plot"><svg viewBox="0 0 {WIDTH} 100" preserveAspectRatio="none" aria-hidden="true">{svg}</svg><div class="lat-chart__cols" style="grid-template-columns:repeat({n},1fr)" aria-hidden="true">{columns}</div>{empty}</div><div class="lat-chart__x" style="grid-template-columns:repeat({n},1fr)" aria-hidden="true">{dates}</div></div><table class="lat-chart__data"><thead><tr><td></td>{heads}</tr></thead><tbody>{table}</tbody></table></figure>"#
        ))
    }
}

/// One figure in a row of [`stats`]: what it is, its value, and a note under it.
#[derive(Clone, Debug)]
pub struct Stat {
    label: Text,
    value: String,
    note: Option<Text>,
}

impl Stat {
    /// A figure: `label` over `value`, which is written as given.
    pub fn new(label: impl Into<Text>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            note: None,
        }
    }

    /// Sets the line under the value.
    pub fn note(mut self, note: impl Into<Text>) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// Renders a row of figures, for the totals above a page's charts.
pub fn stats(shell: &Shell, stats: &[Stat]) -> Markup {
    let cells: String = stats
        .iter()
        .map(|s| {
            let note = s.note.as_ref().map_or(String::new(), |n| {
                format!("<small>{}</small>", escape_attr(&shell.tt(n)))
            });
            format!(
                r#"<div class="lat-stat"><span>{}</span><b>{}</b>{note}</div>"#,
                escape_attr(&shell.tt(&s.label)),
                escape_attr(&s.value),
            )
        })
        .collect();
    Markup::from_override(format!(r#"<div class="lat-stats">{cells}</div>"#))
}

impl Format {
    /// Writes `value` as a chart with this format does.
    pub fn text(&self, value: f64) -> String {
        self.write(value)
    }
}

/// The scale's top for a tallest value: the next of 1, 2, 4, 5, 8 and 10 times
/// a power of ten, so the four steps under it read cleanly.
fn clean_top(tallest: f64) -> f64 {
    if tallest <= 0.0 {
        return 4.0;
    }
    let power = 10f64.powf(tallest.log10().floor());
    let lead = tallest / power;
    let step = [1.0, 2.0, 4.0, 5.0, 8.0, 10.0]
        .into_iter()
        .find(|s| lead <= *s)
        .unwrap_or(10.0);
    // Whole counts keep whole steps.
    (step * power).max(if tallest.fract() == 0.0 { 4.0 } else { 0.0 })
}

/// A number with thousands separators and a fixed count of decimals.
fn grouped(value: f64, decimals: usize) -> String {
    let text = format!("{value:.decimals$}");
    let (whole, rest) = text.split_once('.').unwrap_or((&text, ""));
    let (sign, digits) = whole
        .strip_prefix('-')
        .map_or(("", whole), |digits| ("-", digits));
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    if rest.is_empty() {
        format!("{sign}{out}")
    } else {
        format!("{sign}{out}.{rest}")
    }
}

/// An axis value in few characters: 1.5k, 2M.
fn short(value: f64) -> String {
    let (scaled, unit) = if value >= 1_000_000.0 {
        (value / 1_000_000.0, "M")
    } else if value >= 1_000.0 {
        (value / 1_000.0, "k")
    } else {
        (value, "")
    };
    let text = format!("{scaled:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    format!("{text}{unit}")
}

/// Percent-encodes a query key or value.
fn encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b',' => {
                out.push(char::from(byte));
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn query(pairs: &[(&str, &str)]) -> ChartQuery {
        let text: Vec<String> = pairs.iter().map(|(k, v)| format!("{k}={v}")).collect();
        let uri: axum::http::Uri = format!("/?{}", text.join("&")).parse().unwrap();
        axum::extract::Query::<ChartQuery>::try_from_uri(&uri)
            .unwrap()
            .0
    }

    #[test]
    fn a_query_with_nothing_is_the_last_thirty_days_by_day() {
        let period = Period::from_query(&ChartQuery::default(), day(2026, 3, 31));
        assert_eq!(
            (period.from(), period.to()),
            (day(2026, 3, 2), day(2026, 3, 31))
        );
        assert_eq!(period.interval(), Interval::Day);
        assert_eq!(period.points().len(), 30);
        assert_eq!(period.query(), "range=30d");
    }

    #[test]
    fn presets_and_custom_ranges_resolve_and_round_trip() {
        let today = day(2026, 3, 15);
        let last = Period::from_query(&query(&[("range", "last_month")]), today);
        assert_eq!(
            (last.from(), last.to()),
            (day(2026, 2, 1), day(2026, 2, 28))
        );
        let year = Period::from_query(&query(&[("range", "12m")]), today);
        assert_eq!(year.from(), day(2025, 4, 1));
        assert_eq!(year.interval(), Interval::Month);
        assert_eq!(year.points().len(), 12);
        // A reversed custom range is put in order; the interval follows its length.
        let custom = Period::from_query(
            &query(&[
                ("range", "custom"),
                ("from", "2026-03-01"),
                ("to", "2025-09-01"),
            ]),
            today,
        );
        assert_eq!(
            (custom.from(), custom.to()),
            (day(2025, 9, 1), day(2026, 3, 1))
        );
        assert_eq!(custom.interval(), Interval::Week);
        assert_eq!(custom.query(), "range=custom&from=2025-09-01&to=2026-03-01");
        let by = Period::from_query(&query(&[("range", "90d"), ("by", "week")]), today);
        assert_eq!(by.query(), "range=90d&by=week");
        // An unreadable date falls back to the default.
        let bad = Period::from_query(&query(&[("range", "custom"), ("from", "x")]), today);
        assert_eq!(bad.query(), "range=30d");
    }

    #[test]
    fn the_previous_period_has_the_same_length_and_ends_the_day_before() {
        let period = Period::from_query(&query(&[("range", "7d")]), day(2026, 3, 15));
        let before = period.previous();
        assert_eq!(
            (before.from(), before.to()),
            (day(2026, 3, 2), day(2026, 3, 8))
        );
    }

    #[test]
    fn dated_rows_land_in_their_point_and_gaps_are_zero() {
        let period = Period::from_query(
            &query(&[
                ("range", "custom"),
                ("from", "2026-03-02"),
                ("to", "2026-03-15"),
                ("by", "week"),
            ]),
            day(2026, 3, 15),
        );
        let rows = vec![
            (day(2026, 3, 2), 1.0),
            (day(2026, 3, 8), 2.0),
            (day(2026, 3, 9), 4.0),
            (day(2026, 2, 1), 100.0),
        ];
        let series = Series::daily("calls", "Calls", &period, rows);
        assert_eq!(series.values, vec![3.0, 4.0]);
        assert_eq!(series.total(), 7.0);
    }

    #[test]
    fn values_are_written_grouped_and_short() {
        assert_eq!(Format::Number.write(1234567.0), "1,234,567");
        assert_eq!(Format::Number.write(12.5), "12.50");
        assert_eq!(Format::Money("INR".into()).write(1234.5), "INR 1,234.50");
        assert_eq!(short(1500.0), "1.5k");
        assert_eq!(short(2_000_000.0), "2M");
        assert_eq!(short(7.5), "7.5");
        assert_eq!(clean_top(3.0), 4.0);
        assert_eq!(clean_top(93.0), 100.0);
        assert_eq!(clean_top(0.0), 4.0);
        assert_eq!(clean_top(0.3), 0.4);
    }

    fn chart(period: &Period) -> Chart {
        Chart::new("usage", period.clone())
            .title("Usage")
            .series(Series::new("marketing", "Marketing", vec![1.0; 7]))
            .series(Series::new("utility", "Utility", vec![3.0; 7]))
    }

    #[test]
    fn a_chart_draws_every_series_and_links_each_chip_to_hide_it() {
        let shell = Shell::test();
        let q = query(&[("range", "7d")]);
        let period = Period::from_query(&q, day(2026, 3, 15));
        let html = chart(&period).render(&shell, &q).into_string();
        assert!(
            html.starts_with(r#"<figure class="lat-chart" id="usage">"#),
            "{html}"
        );
        assert_eq!(html.matches("lat-chart__line").count(), 2, "{html}");
        assert_eq!(html.matches("lat-chart__col").count() - 1, 7, "{html}");
        // Hiding Marketing leaves Utility; nothing is hidden yet, so no Show all.
        assert!(
            html.contains(r#"href="?range=7d&amp;show_usage=utility""#),
            "{html}"
        );
        assert!(!html.contains("lat-chart__all"), "{html}");
        assert!(html.contains("<b>21</b>"), "{html}");
        // The table carries the same numbers for a screen reader.
        assert!(
            html.contains(r#"<th scope="row">9 Mar</th><td>1</td><td>3</td>"#),
            "{html}"
        );
    }

    #[test]
    fn a_hidden_series_is_left_out_and_the_scale_follows_what_shows() {
        let shell = Shell::test();
        let q = query(&[
            ("range", "7d"),
            ("show_usage", "marketing"),
            ("show_cost", "a"),
        ]);
        let period = Period::from_query(&q, day(2026, 3, 15));
        let html = chart(&period).render(&shell, &q).into_string();
        assert_eq!(html.matches("lat-chart__line").count(), 1, "{html}");
        assert!(html.contains("lat-chart__fill"), "{html}");
        assert!(html.contains("is-off"), "{html}");
        // The other chart's choice rides along; Show all drops this chart's.
        assert!(
            html.contains(r#"class="lat-chart__all" href="?range=7d&amp;show_cost=a""#),
            "{html}"
        );
        // The last series left links to itself.
        assert!(
            html.contains(r#"href="?range=7d&amp;show_cost=a&amp;show_usage=marketing""#),
            "{html}"
        );
        assert!(html.contains("<span>4</span>"), "{html}");
    }

    #[test]
    fn stacked_bars_scale_to_the_sum_and_an_empty_chart_says_so() {
        let shell = Shell::test();
        let q = query(&[("range", "7d")]);
        let period = Period::from_query(&q, day(2026, 3, 15));
        let bars = chart(&period).kind(Kind::StackedBar).render(&shell, &q);
        let html = bars.into_string();
        assert_eq!(html.matches("<rect").count(), 14, "{html}");
        assert!(!html.contains("lat-chart__dot"), "{html}");
        let empty = Chart::new("none", period.clone())
            .series(Series::new("a", "A", Vec::new()))
            .render(&shell, &q)
            .into_string();
        assert!(empty.contains("Nothing in this period."), "{empty}");
    }

    #[test]
    fn the_period_bar_marks_the_preset_and_keeps_shown_series() {
        let shell = Shell::test();
        let q = query(&[("range", "90d"), ("show_usage", "utility")]);
        let period = Period::from_query(&q, day(2026, 3, 15));
        let html = period.bar(&shell, &q).into_string();
        assert!(
            html.contains(
                r#"<a class="lat-period__preset is-on" href="?range=90d&amp;show_usage=utility">90 days</a>"#
            ),
            "{html}"
        );
        assert!(
            html.contains(r#"<input type="hidden" name="show_usage" value="utility">"#),
            "{html}"
        );
        assert!(html.contains(r#"name="from" value="2025-12-16""#), "{html}");
    }
}
