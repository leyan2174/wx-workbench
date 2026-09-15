//! WeChat location field decoding; presentation and JSON stay outside business.
use crate::business::structured_message::{LocationContent, LocationSummary};
use std::collections::BTreeMap;

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

pub fn parse(xml: &str) -> Option<LocationContent> {
    let doc = crate::message::xml::parse(xml)?;
    let node = doc
        .root_element()
        .descendants()
        .skip(1)
        .find(|node| node.has_tag_name("location"))?;
    let fields: BTreeMap<String, String> = TEXT_FIELDS
        .iter()
        .map(|name| {
            (
                (*name).to_owned(),
                crate::message::xml::collapse(node.attribute(*name).unwrap_or("")),
            )
        })
        .collect();
    let coordinate = |name| {
        node.attribute(name)?
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
    };
    let category_top = fields["poiCategoryTips"]
        .split(':')
        .next()
        .unwrap_or("")
        .to_owned();
    Some(LocationContent {
        summary: LocationSummary {
            name: fields["poiname"].clone(),
            address: fields["label"].clone(),
            category: category_top,
        },
        // WeChat x is latitude and y is longitude.
        latitude: coordinate("x"),
        longitude: coordinate("y"),
        point_id: fields["poiid"].clone(),
        category_tips: fields["poiCategoryTips"].clone(),
        business_hours: fields["poiBusinessHour"].clone(),
        phone: fields["poiPhone"].clone(),
        price_tips: fields["poiPriceTips"].clone(),
        from_point_list: fields["isFromPoiList"].clone(),
        city: fields["cityname"].clone(),
        administrative_code: fields["adcode"].clone(),
        building: fields["buildingId"].clone(),
        floor: fields["floorName"].clone(),
        info_url: fields["infourl"].clone(),
        map_type: fields["maptype"].clone(),
        map_scale: fields["scale"].clone(),
        sender: fields["fromusername"].clone(),
        version: fields["version"].clone(),
    })
}
