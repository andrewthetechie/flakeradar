//! The wire readers a timestamp, a boolean or an integer field goes through.
//!
//! These are the readers a body or query field names in its serde attributes, so
//! the paths they live at are part of the wire: they are re-exported from the
//! dto root, and no reader is renamed here.

// Rust guideline compliant 2026-07-21

/// The timestamp shape every response uses.
///
/// A timestamp is `YYYY-MM-DDTHH:MM:SS`, then `.` and six fractional digits,
/// then `Z`. Fabro rejects anything else, so the fraction is always written,
/// even when it is zero.
pub mod timestamp {
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    /// The `strftime` pattern every timestamp is written with.
    pub const FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.6fZ";

    /// Writes a timestamp in the one accepted shape.
    ///
    /// # Errors
    ///
    /// Returns the serializer's error when the sink rejects the string.
    pub fn serialize<S: Serializer>(
        value: &DateTime<Utc>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.format(FORMAT).to_string())
    }

    /// Reads a timestamp written as RFC 3339.
    ///
    /// # Errors
    ///
    /// Returns an error when the text is not an RFC 3339 timestamp.
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<DateTime<Utc>, D::Error> {
        let text = String::deserialize(deserializer)?;
        DateTime::parse_from_rfc3339(&text)
            .map(|parsed| parsed.with_timezone(&Utc))
            .map_err(serde::de::Error::custom)
    }
}

/// The timestamp helpers for an optional field.
pub mod optional_timestamp {
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    /// Writes an optional timestamp in the one accepted shape.
    ///
    /// # Errors
    ///
    /// Returns the serializer's error when the sink rejects the string.
    pub fn serialize<S: Serializer>(
        value: &Option<DateTime<Utc>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(value) => super::timestamp::serialize(value, serializer),
            None => serializer.serialize_none(),
        }
    }

    /// Reads an optional RFC 3339 timestamp.
    ///
    /// # Errors
    ///
    /// Returns an error when the text is not an RFC 3339 timestamp.
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<DateTime<Utc>>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|text| {
                DateTime::parse_from_rfc3339(&text)
                    .map(|parsed| parsed.with_timezone(&Utc))
                    .map_err(serde::de::Error::custom)
            })
            .transpose()
    }
}

/// The lax boolean a query or a body may spell, for the quarantine toggle.
///
/// Accepted: JSON `true`/`false`, the numbers `1`/`0` (as integers or as the
/// floats `1.0`/`0.0`) and the strings `on`/`off`, `t`/`f`, `true`/`false`,
/// `y`/`n` and `yes`/`no` in any case. Everything else is refused with one of two
/// kinds: `bool_type` for a value of another JSON type, and `bool_parsing` for a
/// number or string of the right type that spells no boolean. Both kinds reach
/// the client, so both are part of the wire: `{"quarantined": "yes"}` is read as
/// `true` and the answer is the route's 404 for a missing Test, while
/// `{"quarantined": []}` is a 422 of the type kind.
pub mod lax_bool {
    use serde::{Deserialize, Deserializer};
    use serde_json::{Number, Value};

    /// The message a boolean of the right type but the wrong spelling is refused
    /// with, which the request extractors report as `bool_parsing`.
    pub const UNPARSABLE_MESSAGE: &str =
        "Input should be a valid boolean, unable to interpret input";

    /// The message a value of another JSON type is refused with, which the
    /// request extractors report as `bool_type`.
    pub const WRONG_TYPE_MESSAGE: &str = "Input should be a valid boolean";

    /// The float that reads as `false`.
    const ZERO: f64 = 0.0;

    /// The float that reads as `true`.
    const ONE: f64 = 1.0;

    /// The lower bound, exclusive, of the range the float-to-integer conversion
    /// accepts: `i64::MIN` as a `f64`.
    const INTEGER_FLOOR: f64 = -9.223_372_036_854_776e18;

    /// The upper bound, exclusive, of that range: `i64::MAX` plus one as a `f64`.
    const INTEGER_CEILING: f64 = 9.223_372_036_854_776e18;

    /// Reads a required lax boolean.
    ///
    /// # Errors
    ///
    /// Returns an error when the value is not one of the accepted spellings.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
        interpret(&Value::deserialize(deserializer)?).map_err(refusal)
    }

    /// Reads an optional lax boolean, where `null` or an absent value is `None`.
    ///
    /// # Errors
    ///
    /// Returns an error when a present value is not one of the accepted spellings.
    pub fn optional<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<bool>, D::Error> {
        match Value::deserialize(deserializer)? {
            Value::Null => Ok(None),
            value => interpret(&value).map(Some).map_err(refusal),
        }
    }

    /// The `serde` error a refused value becomes, carrying the message so the
    /// request extractors report the matching kind.
    fn refusal<E: serde::de::Error>(refusal: Refusal) -> E {
        match refusal {
            Refusal::WrongType => E::custom(WRONG_TYPE_MESSAGE),
            Refusal::Unparsable => E::custom(UNPARSABLE_MESSAGE),
        }
    }

    /// What the lax reading makes of one JSON value.
    ///
    /// # Errors
    ///
    /// Returns [`Refusal::WrongType`] for a value of another JSON type and
    /// [`Refusal::Unparsable`] for a number or a string with no boolean in it.
    pub fn read(value: &Value) -> Result<bool, Refusal> {
        interpret(value)
    }

    /// Why a value was refused, which is also the error kind the 422 reports.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Refusal {
        /// A value of another JSON type: the `bool_type` kind.
        WrongType,
        /// A number or a string with no boolean in it: the `bool_parsing` kind.
        Unparsable,
    }

    /// What the lax reading makes of one JSON value.
    fn interpret(value: &Value) -> Result<bool, Refusal> {
        match value {
            Value::Bool(flag) => Ok(*flag),
            Value::Number(number) => interpret_number(number),
            Value::String(text) => match text.to_ascii_lowercase().as_str() {
                "0" | "off" | "f" | "false" | "n" | "no" => Ok(false),
                "1" | "on" | "t" | "true" | "y" | "yes" => Ok(true),
                _ => Err(Refusal::Unparsable),
            },
            _ => Err(Refusal::WrongType),
        }
    }

    /// What the lax reading makes of one JSON number.
    ///
    /// A whole number goes through the integer path, which reads `0` and `1` and
    /// refuses the rest as unparsable. A float reads `0.0` and `1.0`, and takes
    /// the integer path when it is whole and fits the conversion the integer rule
    /// makes, so `2.0` is unparsable; every other float, including `1.5`, `NaN`
    /// and an infinity, is a value of the wrong type.
    #[expect(
        clippy::float_cmp_const,
        reason = "the rule reads exactly these floats as booleans: a tolerance would accept a \
                  value it refuses"
    )]
    fn interpret_number(number: &Number) -> Result<bool, Refusal> {
        if let Some(integer) = number.as_i64() {
            return match integer {
                0 => Ok(false),
                1 => Ok(true),
                _ => Err(Refusal::Unparsable),
            };
        }
        if number.as_u64().is_some() {
            // A whole number past `i64::MAX` fits no integer the rule reads.
            return Err(Refusal::WrongType);
        }
        let Some(float) = number.as_f64() else {
            return Err(Refusal::WrongType);
        };
        if float == ZERO {
            Ok(false)
        } else if float == ONE {
            Ok(true)
        } else if float.fract() == ZERO && float > INTEGER_FLOOR && float < INTEGER_CEILING {
            Err(Refusal::Unparsable)
        } else {
            Err(Refusal::WrongType)
        }
    }
}

/// The lax integer a query parameter may spell.
///
/// A query string carries text, and an integer there is read in the spellings
/// this API accepts: `"1_0"` is 10, `"2.0"` is 2, `" 2 "` is 2, and `"true"` or
/// `"0x10"` are none of them. `serde`'s own number reader takes only digits, so
/// every query field that is an integer goes through here.
pub mod lax_int {
    use serde::{Deserialize, Deserializer};

    use crate::lax::{INT_PARSING_MESSAGE, text_int};

    /// Reads a required lax integer.
    ///
    /// # Errors
    ///
    /// Returns the `int_parsing` message for a text that is no integer.
    pub fn value<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
        let text = String::deserialize(deserializer)?;
        text_int(&text).ok_or_else(|| serde::de::Error::custom(INT_PARSING_MESSAGE))
    }

    /// Reads an optional lax integer, where an absent parameter is `None`.
    ///
    /// # Errors
    ///
    /// Returns the `int_parsing` message for a text that is no integer.
    pub fn optional<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<i64>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|text| {
                text_int(&text).ok_or_else(|| serde::de::Error::custom(INT_PARSING_MESSAGE))
            })
            .transpose()
    }
}
