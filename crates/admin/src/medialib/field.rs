//! The `media` field: a file (or files) chosen from the library, stored by
//! id, shown as a tile, picked in a dialog over the library's own browser.

use askama::Template;
use serde::{Deserialize, Serialize};

use crate::field::{
    FieldCx, FieldRegistry, FieldType, FieldVm, OptionsError, ResolvedOptions, SubmittedField,
};
use crate::html::Markup;
use laterite_core::validation::Mode;
use laterite_core::AttrValue;

/// What the picker offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaMode {
    /// Any file in the library.
    #[default]
    File,
    /// Images alone, shown as thumbnails.
    Image,
}

#[derive(Debug, Deserialize, Default)]
struct MediaOptions {
    #[serde(default)]
    mode: MediaMode,
    /// How many files the field holds: `1` (the default) stores one id; any
    /// other number stores a JSON list, `0` meaning no limit.
    #[serde(default = "one")]
    max: u32,
}

fn one() -> u32 {
    1
}

#[derive(Debug, Clone)]
pub(crate) struct MediaResolved {
    mode: MediaMode,
    max: u32,
}

#[derive(Serialize, Deserialize, Default)]
struct MediaData {
    pick_url: String,
    resolve_url: String,
    mode: MediaMode,
    max: u32,
    /// The widget's words, in the operator's locale.
    select: String,
    replace: String,
    clear: String,
    use_selected: String,
    title: String,
}

#[derive(Template)]
#[template(path = "fields/media.html")]
struct MediaTmpl<'a> {
    name: &'a str,
    id: &'a str,
    value: &'a str,
    pick_url: &'a str,
    resolve_url: &'a str,
    mode: &'a str,
    max: u32,
    multiple: bool,
    words: &'a MediaData,
}

/// The `media` field type. Options: `mode: image | file` and `max`.
pub struct MediaField;

/// The ids in a control's value: `"3,7"` or a stored JSON list `[3, 7]`.
fn ids_in(value: &str) -> Vec<i64> {
    let value = value.trim();
    if let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(value) {
        return items.iter().filter_map(|v| v.as_i64()).collect();
    }
    value
        .split(',')
        .filter_map(|p| p.trim().parse::<i64>().ok())
        .collect()
}

impl FieldType for MediaField {
    fn view_key(&self) -> &'static str {
        "media"
    }

    fn option_keys(&self, _raw: &serde_json::Value) -> Option<Vec<String>> {
        Some(vec!["mode".to_string(), "max".to_string()])
    }

    fn resolve_options(
        &self,
        raw: &serde_json::Value,
        _types: &FieldRegistry,
    ) -> Result<ResolvedOptions, OptionsError> {
        let opts: MediaOptions = if raw.is_null() {
            MediaOptions::default()
        } else {
            serde_json::from_value(raw.clone()).map_err(|e| OptionsError(e.to_string()))?
        };
        Ok(ResolvedOptions::new(MediaResolved {
            mode: opts.mode,
            max: opts.max,
        }))
    }

    fn view_model(&self, cx: &FieldCx<'_>) -> FieldVm {
        let resolved = cx.opts.get::<MediaResolved>();
        let data = MediaData {
            pick_url: format!("{}/media/pick", cx.base),
            resolve_url: format!("{}/media/resolve", cx.base),
            mode: resolved.map(|r| r.mode).unwrap_or_default(),
            max: resolved.map(|r| r.max).unwrap_or(1),
            select: cx.i18n.t(&laterite_core::t!("Select")),
            replace: cx.i18n.t(&laterite_core::t!("Replace")),
            clear: cx.i18n.t(&laterite_core::t!("Clear")),
            use_selected: cx.i18n.t(&laterite_core::t!("Use selected")),
            title: cx.i18n.t(&laterite_core::t!("Choose from the library")),
        };
        FieldVm {
            view_key: "media".to_string(),
            name: cx.name.to_string(),
            id: cx.id.to_string(),
            label: cx.label.to_string(),
            required: cx.required,
            value: cx.value.clone(),
            data: serde_json::to_value(data).unwrap_or_default(),
        }
    }

    fn render_default(&self, vm: &FieldVm) -> Markup {
        let data: MediaData = serde_json::from_value(vm.data.clone()).unwrap_or_default();
        let value: String = ids_in(vm.value.as_text())
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",");
        Markup::from_template(&MediaTmpl {
            name: &vm.name,
            id: &vm.id,
            value: &value,
            pick_url: &data.pick_url,
            resolve_url: &data.resolve_url,
            mode: match data.mode {
                MediaMode::Image => "image",
                MediaMode::File => "file",
            },
            max: data.max,
            multiple: data.max != 1,
            words: &data,
        })
        .unwrap_or_default()
    }

    /// One id as a number (null when cleared); several as a JSON list.
    fn to_attr(
        &self,
        field: &SubmittedField<'_>,
        opts: &ResolvedOptions,
        _mode: Mode,
    ) -> Result<Option<AttrValue>, String> {
        let max = opts.get::<MediaResolved>().map(|r| r.max).unwrap_or(1);
        Ok(field.value().map(|v| {
            let mut ids = ids_in(v);
            if max > 1 {
                ids.truncate(max as usize);
            }
            if max == 1 {
                match ids.first() {
                    Some(id) => AttrValue::Int(*id),
                    None => AttrValue::Null,
                }
            } else {
                AttrValue::Json(serde_json::Value::Array(
                    ids.into_iter().map(serde_json::Value::from).collect(),
                ))
            }
        }))
    }

    fn to_control(&self, stored: Option<&str>, _opts: &ResolvedOptions) -> String {
        ids_in(stored.unwrap_or_default())
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",")
    }

    fn linked_media(&self, submitted: Option<&str>, opts: &ResolvedOptions) -> Option<Vec<i64>> {
        let max = opts.get::<MediaResolved>().map(|r| r.max).unwrap_or(1);
        let mut ids = ids_in(submitted.unwrap_or_default());
        if max >= 1 {
            ids.truncate(max as usize);
        }
        Some(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_come_from_a_list_or_a_comma_string() {
        assert_eq!(ids_in("3, 7"), vec![3, 7]);
        assert_eq!(ids_in("[3,7]"), vec![3, 7]);
        assert_eq!(ids_in(""), Vec::<i64>::new());
        assert_eq!(ids_in("x"), Vec::<i64>::new());
    }
}
