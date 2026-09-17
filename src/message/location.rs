//! 位置消息的结构化内容与阅读摘要。字段命名兼容旧解码器。

use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Serialize)]
pub struct Location {
    #[serde(flatten)]
    fields: BTreeMap<String, String>,
    pub lat: Option<f64>,
    pub lng: Option<f64>,
    pub category_top: String,
}

impl From<crate::business::structured_message::LocationContent> for Location {
    fn from(value: crate::business::structured_message::LocationContent) -> Self {
        Self {
            fields: [
                ("label".to_owned(), value.summary.address),
                ("poiname".to_owned(), value.summary.name),
                ("poiid".to_owned(), value.point_id),
                ("poiCategoryTips".to_owned(), value.category_tips),
                ("poiBusinessHour".to_owned(), value.business_hours),
                ("poiPhone".to_owned(), value.phone),
                ("poiPriceTips".to_owned(), value.price_tips),
                ("isFromPoiList".to_owned(), value.from_point_list),
                ("cityname".to_owned(), value.city),
                ("adcode".to_owned(), value.administrative_code),
                ("buildingId".to_owned(), value.building),
                ("floorName".to_owned(), value.floor),
                ("infourl".to_owned(), value.info_url),
                ("maptype".to_owned(), value.map_type),
                ("scale".to_owned(), value.map_scale),
                ("fromusername".to_owned(), value.sender),
                ("version".to_owned(), value.version),
            ]
            .into_iter()
            .collect(),
            lat: value.latitude,
            lng: value.longitude,
            category_top: value.summary.category,
        }
    }
}

impl Location {
    pub fn render(&self) -> String {
        let mut lines = vec!["位置消息:".to_owned()];
        for (key, label) in [
            ("poiname", "POI 名"),
            ("label", "地址"),
            ("poiCategoryTips", "品类"),
            ("poiPhone", "电话"),
            ("poiBusinessHour", "营业时间"),
            ("poiPriceTips", "价格档位"),
            ("cityname", "城市"),
            ("adcode", "行政区划码"),
            ("buildingId", "buildingId"),
            ("floorName", "楼层"),
            ("poiid", "POI id"),
        ] {
            if !self.fields[key].is_empty() {
                lines.push(format!("  {label}: {}", self.fields[key]));
            }
        }
        if !self.fields["isFromPoiList"].is_empty() {
            lines.push(format!(
                "  来源: {} (true/1=用户从 POI 列表选择，false/0=手扔图钉)",
                self.fields["isFromPoiList"]
            ));
        }
        if let (Some(lat), Some(lng)) = (self.lat, self.lng) {
            lines.push(format!(
                "  经纬度: ({lat:.6}, {lng:.6})  # 微信 x→纬度，y→经度"
            ));
        }
        for key in ["infourl", "version"] {
            if !self.fields[key].is_empty() {
                lines.push(format!("  {key}: {}", self.fields[key]));
            }
        }
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Independent wire-format expectation, not the adapter's source-field list.
    const TEXT_FIELDS: &[&str] = &[
        "label",
        "poiname",
        "poiid",
        "poiCategoryTips",
        "poiBusinessHour",
        "poiPhone",
        "poiPriceTips",
        "isFromPoiList",
        "cityname",
        "adcode",
        "buildingId",
        "floorName",
        "infourl",
        "maptype",
        "scale",
        "fromusername",
        "version",
    ];

    fn parse(xml: &str) -> Option<Location> {
        crate::adapters::wechat::messages::location::parse(xml).map(Into::into)
    }

    #[test]
    fn matches_legacy_structured_fields_and_detailed_rendering() {
        let cases: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/location-golden.json"))
                .unwrap();
        for case in cases.as_array().unwrap() {
            let info = parse(case["xml"].as_str().unwrap());
            assert_eq!(serde_json::to_value(&info).unwrap(), case["info"], "{case}");
            assert_eq!(
                serde_json::to_value(info.map(|value| value.render())).unwrap(),
                case["text"],
                "{case}"
            );
        }
    }

    #[test]
    fn preserves_all_text_fields_and_coordinate_direction() {
        let attributes = TEXT_FIELDS
            .iter()
            .enumerate()
            .map(|(i, field)| format!("{field}=' value{i} '"))
            .collect::<Vec<_>>()
            .join(" ");
        let info = parse(&format!(
            "<msg><location {attributes} x='31.2' y='121.5'/></msg>"
        ))
        .unwrap();
        let json = serde_json::to_value(&info).unwrap();
        for (i, field) in TEXT_FIELDS.iter().enumerate() {
            assert_eq!(json[*field], format!("value{i}"));
        }
        assert_eq!(info.lat, Some(31.2));
        assert_eq!(info.lng, Some(121.5));
        assert_eq!(json.as_object().unwrap().len(), TEXT_FIELDS.len() + 3);
    }

    #[test]
    fn missing_and_invalid_values_have_stable_json_shape() {
        for coordinates in ["", "x='bad' y=''", "x='NaN' y='inf'"] {
            let json = serde_json::to_value(
                parse(&format!("<msg><location {coordinates}/></msg>")).unwrap(),
            )
            .unwrap();
            assert!(json["lat"].is_null());
            assert!(json["lng"].is_null());
            for field in TEXT_FIELDS {
                assert_eq!(json[*field], "");
            }
        }
        assert!(parse("<location/>").is_none());
        assert!(parse("<msg/>").is_none());
    }
}
