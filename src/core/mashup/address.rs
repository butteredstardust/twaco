//! The small, deliberately lexical language used to name mashup binding ends.

use crate::core::codes::{Coded, ErrorCode};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Address {
    Ui {
        widget: String,
        property: String,
    },
    Mashup {
        parameter: String,
    },
    /// `entity_service` is resolved against the workspace because both entity names and service
    /// names may contain dots.
    Data {
        entity_service: String,
        alias: Option<String>,
        selected_rows: bool,
        member: Option<String>,
    },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("invalid mashup address `{0}`")]
pub(crate) struct AddressError(pub(crate) String);

impl Coded for AddressError {
    fn code(&self) -> ErrorCode {
        ErrorCode::InvalidArguments
    }
}

/// Parse an address exactly as `mashup describe` prints it.  Resolving the split between a dotted
/// entity name and service is intentionally the caller's job: only its repository can do that.
pub(crate) fn parse_address(text: &str) -> Result<Address, AddressError> {
    let bad = || AddressError(text.to_string());
    let (area, rest) = text.split_once(':').ok_or_else(bad)?;
    if rest.is_empty() || rest.trim() != rest {
        return Err(bad());
    }
    match area {
        "ui" => {
            let (widget, property) = rest.rsplit_once('.').ok_or_else(bad)?;
            if widget.is_empty() || property.is_empty() {
                Err(bad())
            } else {
                Ok(Address::Ui {
                    widget: widget.into(),
                    property: property.into(),
                })
            }
        }
        "mashup" if !rest.contains(['.', '[', ']']) => Ok(Address::Mashup {
            parameter: rest.into(),
        }),
        "data" => data(rest).ok_or_else(bad),
        _ => Err(bad()),
    }
}

fn data(rest: &str) -> Option<Address> {
    // `head[as alias][SelectedRows].member`: either bracket may be left out, in that order.
    let (head, mut tail) = match rest.find('[') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, ""),
    };
    let mut alias = None;
    let mut selected_rows = false;
    if let Some(after) = tail.strip_prefix("[as ") {
        let (name, after) = after.split_once(']')?;
        if name.is_empty() || name.trim() != name || name.contains('[') {
            return None;
        }
        alias = Some(name.to_string());
        tail = after;
    }
    if let Some(after) = tail.strip_prefix("[SelectedRows]") {
        selected_rows = true;
        tail = after;
    }
    if head.is_empty() || !head.contains('.') || head.split('.').any(str::is_empty) {
        return None;
    }
    let member = match tail {
        "" => None,
        after if after.starts_with('.') && after.len() > 1 && !after.contains(['[', ']']) => {
            Some(after[1..].to_string())
        }
        _ => return None,
    };
    Some(Address::Data {
        entity_service: head.to_string(),
        alias,
        selected_rows,
        member,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_described_form() {
        assert_eq!(
            parse_address("ui:ptcs.grid-3.Data").unwrap(),
            Address::Ui {
                widget: "ptcs.grid-3".into(),
                property: "Data".into()
            }
        );
        assert_eq!(
            parse_address("mashup:Filter").unwrap(),
            Address::Mashup {
                parameter: "Filter".into()
            }
        );
        assert_eq!(
            parse_address("data:Acme.Orders.Manager.GetOrders").unwrap(),
            Address::Data {
                entity_service: "Acme.Orders.Manager.GetOrders".into(),
                alias: None,
                selected_rows: false,
                member: None
            }
        );
        assert_eq!(
            parse_address("data:Acme.Orders.Manager.GetOrders.note").unwrap(),
            Address::Data {
                entity_service: "Acme.Orders.Manager.GetOrders.note".into(),
                alias: None,
                selected_rows: false,
                member: None
            }
        );
        assert_eq!(
            parse_address("data:Acme.Orders.Manager.GetOrders[as orders].note").unwrap(),
            Address::Data {
                entity_service: "Acme.Orders.Manager.GetOrders".into(),
                alias: Some("orders".into()),
                selected_rows: false,
                member: Some("note".into())
            }
        );
        assert_eq!(
            parse_address("data:Acme.Orders.Manager.GetOrders[SelectedRows].note").unwrap(),
            Address::Data {
                entity_service: "Acme.Orders.Manager.GetOrders".into(),
                alias: None,
                selected_rows: true,
                member: Some("note".into())
            }
        );
        // Selected rows of a call with its own name, as `mashup describe` prints them.
        assert_eq!(
            parse_address("data:Acme.Orders.Manager.GetOrders[as orders][SelectedRows].note")
                .unwrap(),
            Address::Data {
                entity_service: "Acme.Orders.Manager.GetOrders".into(),
                alias: Some("orders".into()),
                selected_rows: true,
                member: Some("note".into())
            }
        );
    }

    #[test]
    fn rejects_malformed_addresses() {
        for address in [
            "x",
            "ui:x",
            "ui:.P",
            "ui:x.",
            "mashup:",
            "mashup:A.B",
            "data:E",
            "data:.S",
            "data:E.S[as ]",
            "data:E.S[Rows]",
            "data:E.S[]",
            "data:E.S[SelectedRows]x",
            "data:E.S[a]",
            "data:E.S[SelectedRows][as a]",
            "data:E.S[as a][as b]",
        ] {
            assert!(parse_address(address).is_err(), "{address}");
        }
    }
}
