# Charts

A chart draws series of values over a period on a screen: as lines, bars or
stacked bars, with a period bar above and a legend that hides and shows series.

## Draw a chart

```rust
use chrono::Utc;
use laterite_admin::axum::extract::Query;
use laterite_admin::axum::response::Response;
use laterite_admin::axum::Extension;
use laterite_admin::chart::{Chart, ChartQuery, Kind, Period, Series, CHART_ASSETS};
use laterite_admin::{html::Markup, PageHead, Shell};
use laterite_core::t;

async fn orders(
    Query(query): Query<ChartQuery>,
    Extension(shell): Extension<Shell>,
) -> Response {
    let period = Period::from_query(&query, Utc::now().date_naive());
    // (day, value) rows for period.from()..=period.to().
    let web = acme::orders::by_day("web", period.from(), period.to()).await;
    let shop = acme::orders::by_day("shop", period.from(), period.to()).await;

    let chart = Chart::new("orders", period.clone())
        .title(t!("Orders"))
        .kind(Kind::StackedBar)
        .series(Series::daily("web", t!("Website"), &period, web))
        .series(Series::daily("shop", t!("Shop"), &period, shop));

    let mut body = period.bar(&shell, &query).into_string();
    body.push_str(chart.render(&shell, &query).as_str());
    shell.page_with(
        PageHead::new(t!("Orders")).assets(CHART_ASSETS),
        Markup::raw_unchecked(body),
    )
}
```

## Period

`Period::from_query(&query, today)` reads these query parameters.

Parameter | Values | Result
--- | --- | ---
`range` | `7d`, `30d`, `90d` | The last 7, 30 or 90 days. `30d` when absent.
`range` | `month`, `last_month` | This calendar month to today; the whole of last month.
`range` | `12m` | The last 12 calendar months, a point a month.
`range` | `custom` | `from` to `to`, both `YYYY-MM-DD`, at most 1830 days.
`by` | `day`, `week`, `month` | The interval. `day` up to 370 days.

Method | Returns
--- | ---
`from()`, `to()` | The first and last day.
`interval()` | `Interval::Day` up to 92 days, `Week` up to 370, else `Month`, unless `by` set it.
`points()` | The first day of each point.
`previous()` | The period of the same length ending the day before.
`query()` | The period as query parameters, for a link.
`bar(&shell, &query)` | The period bar: the presets and a from and to date.

## Series

```rust
// Dated rows: summed into the period's points, a missing point is zero.
Series::daily("web", t!("Website"), &period, vec![(day, 12.0)]);
// One value a point, in order.
Series::new("web", t!("Website"), vec![12.0, 9.0, 14.0]);
```

## Chart

Method | Default | Effect
--- | --- | ---
`Chart::new(id, period)` | | `id` names the chart on the page: letters, digits, `-`, `_`.
`.kind(Kind)` | `Kind::Line` | `Line`, `Bar` or `StackedBar`.
`.format(Format)` | `Format::Number` | `Number`, or `Money("EUR".into())` for `EUR 1,234.50`.
`.title(text)` | none | The heading above the legend.
`.series(series)` | | Adds a series; its colour follows its place.
`.render(&shell, &query)` | | The chart, showing the series `show_<id>` names.

## Figures

```rust
use laterite_admin::chart::{stats, Format, Stat};

let row = stats(&shell, &[
    Stat::new(t!("Orders"), Format::Number.text(1284.0)).note(t!("In this period")),
    Stat::new(t!("Revenue"), Format::Money("EUR".into()).text(9120.5)),
]);
```

## Colours

A skin sets `--chart-1` to `--chart-6`.

```css
:root { --chart-1: #0b6e4f; --chart-2: #c8553d; }
```
