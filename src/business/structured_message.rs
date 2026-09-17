//! Read-only message content, independent of storage and protocol transports.

/// A missing preview does not mean the original message is missing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentIssue {
    UnsupportedKind,
    InputTooLarge,
    MalformedContent,
    NoSafePreview,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StructuredMessage {
    Link {
        title: String,
        des: String,
        url: String,
        source: String,
    },
    File {
        title: String,
        file_ext: String,
        file_size: u64,
    },
    Miniapp {
        title: String,
        source: String,
        url: String,
    },
    Channels {
        title: String,
    },
    Chatlog {
        title: String,
        des: String,
        items: Vec<ChatItem>,
    },
    Quote {
        title: String,
        ref_name: String,
        ref_content: String,
    },
    Transfer {
        title: String,
        status: TransferStatus,
        // Source evidence only; business state is always represented by status.
        raw_subtype: String,
        amount_text: String,
        memo: String,
    },
    Voice {
        duration: f64,
    },
    Video {
        duration: u64,
    },
    Emoji {
        emoji_url: String,
        md5: String,
    },
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ChatItem {
    pub name: String,
    pub text: String,
}

/// Complete transfer metadata. Amounts and timestamps retain their source text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferDetails {
    pub raw_subtype: String,
    pub status: TransferStatus,
    pub amount_text: String,
    pub memo: String,
    pub transaction_id: String,
    pub transfer_id: String,
    pub payment_message_id: String,
    pub started_at: String,
    pub expires_at: String,
    pub effective_date: String,
    pub payer: String,
    pub receiver: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferStatus {
    Initiated,
    Received,
    Returned,
    ExpiredReturned,
    PendingCollection,
    Collected,
    Missing,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferContent {
    pub title: String,
    pub description: String,
    pub details: TransferDetails,
}

/// Reading summaries preserve non-numeric duration text independently
/// of the bounded, numeric rich preview above.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallSummary {
    Empty,
    Duration(String),
    Canceled,
    Busy,
    AnsweredElsewhere,
    DeclinedElsewhere,
    CanceledByCaller,
    NotAnswered,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamecardSummary {
    pub nickname: String,
    pub username: String,
    pub is_public_account: bool,
    pub biography: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocationSummary {
    pub name: String,
    pub address: String,
    pub category: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LocationContent {
    pub summary: LocationSummary,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub point_id: String,
    pub category_tips: String,
    pub business_hours: String,
    pub phone: String,
    pub price_tips: String,
    pub from_point_list: String,
    pub city: String,
    pub administrative_code: String,
    pub building: String,
    pub floor: String,
    pub info_url: String,
    pub map_type: String,
    pub map_scale: String,
    pub sender: String,
    pub version: String,
}
