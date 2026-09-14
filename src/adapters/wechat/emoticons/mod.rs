//! Materialized WeChat emoticon catalog and private resource material.
pub(crate) mod catalog;
pub(crate) mod types;
use crate::{business::emoticons as domain, daemon::cache::DbCache};
use domain::{CatalogMediaRef, Error, Failure, Stage};
use std::sync::Arc;

pub(crate) struct CatalogSource {
    owner: Arc<()>,
    raw: types::Catalog,
}
impl CatalogSource {
    pub(crate) async fn load(cache: &DbCache) -> anyhow::Result<Self> {
        Ok(Self::from_raw(catalog::load(cache).await?))
    }
    pub(crate) fn from_path(path: &std::path::Path) -> anyhow::Result<Self> {
        Ok(Self::from_raw(catalog::load_from_path(path)?))
    }
    fn from_raw(raw: types::Catalog) -> Self {
        Self {
            owner: Arc::new(()),
            raw,
        }
    }
    pub(crate) fn find(&self, md5: &str) -> Result<Option<CatalogMediaRef>, Error> {
        if !self.raw.source_available {
            return Err(Error::new(Stage::Discovery, Failure::NotFound));
        }
        let mut matches = self
            .raw
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.md5.eq_ignore_ascii_case(md5));
        let found = matches
            .next()
            .map(|(slot, _)| CatalogMediaRef::new(&self.owner, slot));
        if matches.next().is_some() {
            return Err(Error::new(Stage::Association, Failure::Ambiguous));
        }
        Ok(found)
    }
    pub(crate) fn revalidate(&self, reference: &CatalogMediaRef) -> Result<(), Error> {
        self.material(reference).map(|_| ())
    }
    pub(crate) fn material(&self, reference: &CatalogMediaRef) -> Result<&types::Emoji, Error> {
        let slot = reference.slot_for(&self.owner)?;
        self.raw
            .items
            .get(slot)
            .ok_or(Error::new(Stage::Revalidation, Failure::InvalidReference))
    }
}
impl domain::Source for CatalogSource {
    fn catalog(&self) -> Result<domain::Catalog, Error> {
        if !self.raw.source_available {
            return Err(Error::new(Stage::Discovery, Failure::NotFound));
        }
        let items = self
            .raw
            .items
            .iter()
            .enumerate()
            .map(|(slot, row)| domain::Emoticon {
                reference: CatalogMediaRef::new(&self.owner, slot),
                id: row.md5.clone(),
                caption: row.info.caption.clone(),
                package: row.info.product_id.clone(),
                origin: if slot < self.raw.non_store_count {
                    domain::Origin::CatalogRecorded
                } else {
                    domain::Origin::TemplateDerived
                },
                has_direct_resource: !row.info.cdn_url.is_empty(),
            })
            .collect();
        Ok(domain::Catalog {
            items,
            recorded_count: self.raw.non_store_count,
            derived_count: self.raw.store_added,
        })
    }
    fn revalidate(&self, reference: &CatalogMediaRef) -> Result<(), Error> {
        self.revalidate(reference)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::Source;
    #[test]
    fn materialized_catalog_keeps_origin_order_private_material_and_scope() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("catalog.db");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(include_str!(
                "../../../../tests/fixtures/emoticons-catalog/schema.sql"
            ))
            .unwrap();
        connection.execute_batch("INSERT INTO kNonStoreEmoticonTable VALUES('b','old','https://example.invalid?m=abc&v=1','','p'),('a','','','',''),('b','synthetic-private-key','https://example.invalid?m=abc&secret=synthetic-private-url','','p'); INSERT INTO kStoreEmoticonFilesTable VALUES('p','b'),('p','c')").unwrap();
        let source = CatalogSource::from_path(&path).unwrap();
        let catalog = source.catalog().unwrap();
        assert_eq!(
            catalog
                .items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["b", "a", "c"]
        );
        assert_eq!(catalog.items[0].origin, domain::Origin::CatalogRecorded);
        assert_eq!(catalog.items[2].origin, domain::Origin::TemplateDerived);
        assert_eq!(
            source
                .material(&catalog.items[0].reference)
                .unwrap()
                .info
                .aes_key,
            "synthetic-private-key"
        );
        assert!(!format!("{:?}", catalog.items).contains("synthetic-private"));
        let another = CatalogSource::from_path(&path).unwrap();
        assert!(another.revalidate(&catalog.items[0].reference).is_err());
        assert_eq!(
            source.find("C").unwrap().unwrap(),
            catalog.items[2].reference
        );
        drop(source);
        assert!(catalog.items[0].reference.is_expired());
    }
}
