//! Materialized emoticon catalogs. References are not message identities or authorization.
pub use super::media::{Error, Failure, Stage};
use std::{
    fmt,
    sync::{Arc, Weak},
};

#[derive(Clone)]
pub struct CatalogMediaRef {
    owner: Weak<()>,
    slot: usize,
}
impl CatalogMediaRef {
    pub(crate) fn new(owner: &Arc<()>, slot: usize) -> Self {
        Self {
            owner: Arc::downgrade(owner),
            slot,
        }
    }
    pub fn is_expired(&self) -> bool {
        self.owner.strong_count() == 0
    }
    pub(crate) fn slot_for(&self, owner: &Arc<()>) -> Result<usize, Error> {
        if self
            .owner
            .upgrade()
            .is_some_and(|value| Arc::ptr_eq(&value, owner))
        {
            Ok(self.slot)
        } else {
            Err(Error::new(Stage::Revalidation, Failure::StaleEvidence))
        }
    }
}
impl fmt::Debug for CatalogMediaRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CatalogMediaRef")
            .field("expired", &self.is_expired())
            .finish()
    }
}
impl PartialEq for CatalogMediaRef {
    fn eq(&self, other: &Self) -> bool {
        self.owner.ptr_eq(&other.owner) && self.slot == other.slot
    }
}
impl Eq for CatalogMediaRef {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    CatalogRecorded,
    TemplateDerived,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Emoticon {
    pub reference: CatalogMediaRef,
    pub id: String,
    pub caption: Option<String>,
    pub package: String,
    pub origin: Origin,
    pub has_direct_resource: bool,
}

pub struct Catalog {
    pub items: Vec<Emoticon>,
    pub recorded_count: usize,
    pub derived_count: usize,
}

pub trait Source {
    fn catalog(&self) -> Result<Catalog, Error>;
    #[expect(
        dead_code,
        reason = "Source contract requires reference revalidation; current exporter calls the concrete adapter method"
    )]
    fn revalidate(&self, reference: &CatalogMediaRef) -> Result<(), Error>;
}

/// This legacy export never claims a content hash proof from its filename cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Materialization {
    Downloaded,
    LegacyCache,
    Converted,
    ConversionFallback,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exported {
    pub bytes: u64,
    pub materialization: Materialization,
}

pub trait Exporter {
    fn export(&mut self, reference: &CatalogMediaRef) -> Result<Exported, Error>;
}

pub struct ItemResult {
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Per-item evidence and outcomes remain observable independently of legacy aggregate counters"
        )
    )]
    pub reference: CatalogMediaRef,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Per-item evidence and outcomes remain observable independently of legacy aggregate counters"
        )
    )]
    pub result: Result<Exported, Error>,
}
pub struct BatchReport {
    pub items: Vec<ItemResult>,
    pub succeeded: usize,
    pub failed: usize,
}

pub fn select(catalog: Catalog, filter: Option<&str>) -> Vec<Emoticon> {
    let filter = filter
        .filter(|text| !text.is_empty())
        .map(str::to_lowercase);
    catalog
        .items
        .into_iter()
        .filter(|item| {
            filter.as_ref().is_none_or(|text| {
                item.caption
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(text)
                    || item.package.to_lowercase().contains(text)
            })
        })
        .collect()
}

/// Per-item failure is observable and does not abandon subsequent items.
pub fn export_batch(items: &[Emoticon], exporter: &mut impl Exporter) -> BatchReport {
    let mut report = BatchReport {
        items: Vec::with_capacity(items.len()),
        succeeded: 0,
        failed: 0,
    };
    for item in items {
        let result = if item.reference.is_expired() {
            Err(Error::new(Stage::Revalidation, Failure::StaleEvidence))
        } else {
            exporter.export(&item.reference)
        };
        if result.is_ok() {
            report.succeeded += 1;
        } else {
            report.failed += 1;
        }
        report.items.push(ItemResult {
            reference: item.reference.clone(),
            result,
        });
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    fn item(owner: &Arc<()>, slot: usize, caption: &str, package: &str) -> Emoticon {
        Emoticon {
            reference: CatalogMediaRef::new(owner, slot),
            id: format!("item{slot}"),
            caption: Some(caption.into()),
            package: package.into(),
            origin: Origin::CatalogRecorded,
            has_direct_resource: true,
        }
    }
    #[test]
    fn selection_preserves_order_casefolding_and_package_matching() {
        let owner = Arc::new(());
        let catalog = Catalog {
            items: vec![
                item(&owner, 0, "ALPHA", ""),
                item(&owner, 1, "other", "Alpha"),
                item(&owner, 2, "other", ""),
            ],
            recorded_count: 3,
            derived_count: 0,
        };
        let selected = select(catalog, Some("alpha"));
        assert_eq!(
            selected
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["item0", "item1"]
        );
    }
    #[test]
    fn catalog_references_cannot_rebind_and_expire_without_registry() {
        let owner = Arc::new(());
        let reference = CatalogMediaRef::new(&owner, 3);
        assert_eq!(reference.slot_for(&owner), Ok(3));
        assert!(reference.slot_for(&Arc::new(())).is_err());
        drop(owner);
        assert!(reference.is_expired());
    }
    struct MemoryExporter {
        calls: usize,
    }
    impl Exporter for MemoryExporter {
        fn export(&mut self, _: &CatalogMediaRef) -> Result<Exported, Error> {
            self.calls += 1;
            if self.calls == 2 {
                Err(Error::new(Stage::Publication, Failure::Refused))
            } else {
                Ok(Exported {
                    bytes: 4,
                    materialization: Materialization::LegacyCache,
                })
            }
        }
    }
    #[test]
    fn batch_keeps_partial_failure_and_continues_in_order() {
        let owner = Arc::new(());
        let items: Vec<_> = (0..3).map(|slot| item(&owner, slot, "", "")).collect();
        let mut exporter = MemoryExporter { calls: 0 };
        let report = export_batch(&items, &mut exporter);
        assert_eq!((report.succeeded, report.failed, exporter.calls), (2, 1, 3));
        assert!(report.items[1].result.is_err());
        assert_eq!(report.items[2].reference, items[2].reference);
    }
    #[test]
    fn expired_reference_never_reaches_execution() {
        let owner = Arc::new(());
        let items = [item(&owner, 0, "", "")];
        drop(owner);
        let mut exporter = MemoryExporter { calls: 0 };
        assert_eq!(export_batch(&items, &mut exporter).failed, 1);
        assert_eq!(exporter.calls, 0);
    }
}
