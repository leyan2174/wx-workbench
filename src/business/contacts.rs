//! Account-scoped contact use cases. No storage, transport or host dependencies.
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContactId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactKind {
    Person,
    Group,
    Official,
    Folded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contact {
    pub id: ContactId,
    /// A materialized preferred name need not claim to be a nickname or remark.
    pub display_name: Option<String>,
    pub nickname: Option<String>,
    pub remark: Option<String>,
    pub alias: Option<String>,
    pub description: Option<String>,
    pub phone: Option<String>,
    pub kind: ContactKind,
    pub verified: Option<bool>,
    pub visible: bool,
}

impl Contact {
    pub fn display(&self) -> &str {
        self.display_name
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                preferred_name(&self.id.0, self.nickname.as_deref(), self.remark.as_deref())
            })
    }
}

pub fn preferred_name<'a>(
    identity: &'a str,
    nickname: Option<&'a str>,
    remark: Option<&'a str>,
) -> &'a str {
    remark
        .filter(|s| !s.is_empty())
        .or_else(|| nickname.filter(|s| !s.is_empty()))
        .unwrap_or(identity)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Unavailable,
    Unsupported(&'static str),
    InvalidData(&'static str),
    NotFound,
    Ambiguous,
    NotGroup,
    Limit,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => f.write_str("contact source unavailable"),
            Self::Unsupported(capability) => {
                write!(f, "unsupported contact capability: {capability}")
            }
            Self::InvalidData(reason) => write!(f, "invalid contact data: {reason}"),
            Self::NotFound => f.write_str("contact or tag not found"),
            Self::Ambiguous => f.write_str("ambiguous contact or tag name"),
            Self::NotGroup => f.write_str("contact is not a group"),
            Self::Limit => f.write_str("contact query limit exceeded"),
        }
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub names: bool,
    pub classification: bool,
    pub labels: bool,
    pub full_membership: bool,
}

#[derive(Debug, Clone)]
pub struct Directory {
    pub contacts: Vec<Contact>,
    pub capabilities: Capabilities,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub id: ContactId,
    pub contact_display: String,
    pub group_nickname: Option<String>,
    pub is_owner: bool,
}
impl Member {
    pub fn display(&self) -> &str {
        self.group_nickname
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.contact_display)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipCoverage {
    Complete,
    ObservedSenders,
}
#[derive(Debug, Clone)]
pub struct Membership {
    pub members: Vec<Member>,
    pub coverage: MembershipCoverage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagMember {
    pub id: ContactId,
    pub display_name: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    pub members: Vec<TagMember>,
}

/// One fixed account per source; IDs are never nicknames or physical row numbers.
pub trait ContactSource {
    fn contacts(&self) -> Result<Directory>;
    fn members(&self, group: &ContactId) -> Result<Membership>;
    fn tags(&self) -> Result<Vec<Tag>>;
}

/// A materialized contact directory is sufficient for listing and identity selection.
impl ContactSource for Directory {
    fn contacts(&self) -> Result<Directory> {
        if self.contacts.is_empty() {
            Err(Error::Unavailable)
        } else {
            Ok(self.clone())
        }
    }
    fn members(&self, _: &ContactId) -> Result<Membership> {
        Err(Error::Unsupported("group membership"))
    }
    fn tags(&self) -> Result<Vec<Tag>> {
        Err(Error::Unsupported("contact labels"))
    }
}

pub struct ContactQuery<'a> {
    pub text: Option<&'a str>,
    pub offset: usize,
    pub limit: usize,
}
pub struct ContactPage {
    pub contacts: Vec<Contact>,
    pub total: usize,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "Typed continuation offset is retained while contact wire only projects total and items"
        )
    )]
    pub next_offset: Option<usize>,
}

pub fn list(source: &impl ContactSource, query: ContactQuery<'_>) -> Result<ContactPage> {
    validate_query(query.text.unwrap_or(""))?;
    let directory = source.contacts()?;
    if !directory.capabilities.classification {
        return Err(Error::Unsupported("contact classification"));
    }
    let needle = query.text.unwrap_or("").to_lowercase();
    let mut contacts: Vec<_> = directory
        .contacts
        .into_iter()
        .filter(|contact| {
            contact.kind == ContactKind::Person
                && (needle.is_empty()
                    || contact.id.0.to_lowercase().contains(&needle)
                    || contact.display().to_lowercase().contains(&needle))
        })
        .collect();
    contacts.sort_by(|a, b| a.display().cmp(b.display()).then(a.id.cmp(&b.id)));
    let total = contacts.len();
    let contacts: Vec<_> = contacts
        .into_iter()
        .skip(query.offset)
        .take(query.limit)
        .collect();
    let end = query.offset.saturating_add(contacts.len());
    Ok(ContactPage {
        contacts,
        total,
        next_offset: (end < total).then_some(end),
    })
}

pub fn validate_query(query: &str) -> Result<()> {
    if query.len() > 4096 {
        Err(Error::Limit)
    } else {
        Ok(())
    }
}

pub fn validate_tag_query(query: &str) -> Result<()> {
    validate_query(query)?;
    if query.trim().is_empty() {
        Err(Error::InvalidData("empty tag query"))
    } else {
        Ok(())
    }
}

/// Exact names take precedence over substrings; ambiguity is never resolved by order.
pub fn select_name<'a>(names: impl IntoIterator<Item = &'a str>, query: &str) -> Result<usize> {
    validate_query(query)?;
    let query = query.trim().to_lowercase();
    let names: Vec<_> = names.into_iter().map(str::to_lowercase).collect();
    let exact: Vec<_> = names
        .iter()
        .enumerate()
        .filter(|(_, name)| *name == &query)
        .map(|(i, _)| i)
        .collect();
    let found = if exact.is_empty() {
        names
            .iter()
            .enumerate()
            .filter(|(_, name)| name.contains(&query))
            .map(|(i, _)| i)
            .collect()
    } else {
        exact
    };
    match found.as_slice() {
        [index] => Ok(*index),
        [] => Err(Error::NotFound),
        _ => Err(Error::Ambiguous),
    }
}

pub fn resolve(source: &impl ContactSource, query: &str) -> Result<Contact> {
    validate_query(query)?;
    let directory = source.contacts()?;
    let identities: Vec<_> = directory
        .contacts
        .iter()
        .filter(|contact| contact.id.0 == query)
        .collect();
    match identities.as_slice() {
        [contact] => return Ok((*contact).clone()),
        [] => (),
        _ => return Err(Error::Ambiguous),
    }
    let index = select_name(directory.contacts.iter().map(Contact::display), query)?;
    Ok(directory.contacts[index].clone())
}

pub fn members(source: &impl ContactSource, query: &str) -> Result<(Contact, Membership)> {
    let group = resolve(source, query)?;
    if group.kind != ContactKind::Group {
        return Err(Error::NotGroup);
    }
    let mut membership = source.members(&group.id)?;
    membership.members.sort_by(|a, b| {
        b.is_owner
            .cmp(&a.is_owner)
            .then(a.display().cmp(b.display()))
            .then(a.id.cmp(&b.id))
    });
    Ok((group, membership))
}

pub fn tag(source: &impl ContactSource, query: &str) -> Result<Tag> {
    validate_tag_query(query)?;
    let tags = source.tags()?;
    let index = select_name(tags.iter().map(|tag| tag.name.as_str()), query)?;
    Ok(tags[index].clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Memory(Vec<Contact>);
    impl ContactSource for Memory {
        fn contacts(&self) -> Result<Directory> {
            Ok(Directory {
                contacts: self.0.clone(),
                capabilities: Capabilities {
                    classification: true,
                    names: true,
                    ..Default::default()
                },
            })
        }
        fn members(&self, _: &ContactId) -> Result<Membership> {
            Ok(Membership {
                members: vec![],
                coverage: MembershipCoverage::ObservedSenders,
            })
        }
        fn tags(&self) -> Result<Vec<Tag>> {
            Ok(vec![
                Tag {
                    name: "Friends".into(),
                    members: vec![],
                },
                Tag {
                    name: "Friends work".into(),
                    members: vec![],
                },
            ])
        }
    }
    fn contact(id: &str, name: &str, kind: ContactKind) -> Contact {
        Contact {
            id: ContactId(id.into()),
            display_name: None,
            nickname: Some(name.into()),
            remark: None,
            alias: None,
            description: None,
            phone: None,
            kind,
            verified: Some(false),
            visible: true,
        }
    }
    #[test]
    fn memory_filter_pagination_and_identity_are_deterministic() {
        let source = Memory(vec![
            contact("b", "Same", ContactKind::Person),
            contact("a", "Same", ContactKind::Person),
            contact("g", "Group", ContactKind::Group),
        ]);
        let page = list(
            &source,
            ContactQuery {
                text: Some("same"),
                offset: 0,
                limit: 1,
            },
        )
        .unwrap();
        assert_eq!((page.total, page.next_offset), (2, Some(1)));
        assert_eq!(page.contacts[0].id.0, "a");
        assert_eq!(resolve(&source, "Same"), Err(Error::Ambiguous));
        assert_eq!(resolve(&source, "a").unwrap().id.0, "a");
        assert!(matches!(members(&source, "a"), Err(Error::NotGroup)));
        assert_eq!(
            members(&source, "g").unwrap().1.coverage,
            MembershipCoverage::ObservedSenders
        );
        assert_eq!(tag(&source, "friends").unwrap().name, "Friends");
        assert_eq!(tag(&source, "missing"), Err(Error::NotFound));
    }
    #[test]
    fn account_sources_do_not_share_identity_or_names() {
        let first = Memory(vec![contact("same-id", "First", ContactKind::Person)]);
        let second = Memory(vec![contact("same-id", "Second", ContactKind::Person)]);
        assert_ne!(
            resolve(&first, "same-id").unwrap(),
            resolve(&second, "same-id").unwrap()
        );
        assert_eq!(select_name(["same", "SAME"], "same"), Err(Error::Ambiguous));
        assert_eq!(validate_query(&"x".repeat(4097)), Err(Error::Limit));
    }

    #[test]
    fn blank_tag_queries_are_rejected_without_changing_name_selection() {
        struct Tags(Vec<Tag>);
        impl ContactSource for Tags {
            fn contacts(&self) -> Result<Directory> {
                unreachable!("tag lookup does not read contacts")
            }
            fn members(&self, _: &ContactId) -> Result<Membership> {
                unreachable!("tag lookup does not read group members")
            }
            fn tags(&self) -> Result<Vec<Tag>> {
                Ok(self.0.clone())
            }
        }
        for count in [1, 2] {
            let source = Tags(Memory(vec![]).tags().unwrap()[..count].to_vec());
            for query in ["", " ", "\t\r\n", "\u{3000}"] {
                assert_eq!(
                    tag(&source, query),
                    Err(Error::InvalidData("empty tag query"))
                );
            }
            assert_eq!(tag(&source, " friends ").unwrap().name, "Friends");
            assert_eq!(tag(&source, "missing"), Err(Error::NotFound));
        }
        let source = Memory(vec![]);
        assert_eq!(tag(&source, "work").unwrap().name, "Friends work");
        assert_eq!(tag(&source, "frien"), Err(Error::Ambiguous));
    }

    #[test]
    fn empty_contact_filter_remains_valid() {
        let source = Memory(vec![contact("a", "Person", ContactKind::Person)]);
        for text in [None, Some("")] {
            let page = list(
                &source,
                ContactQuery {
                    text,
                    offset: 0,
                    limit: 10,
                },
            )
            .unwrap();
            assert_eq!(page.total, 1);
            assert_eq!(page.contacts[0].id.0, "a");
        }
    }
}
