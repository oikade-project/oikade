use std::collections::BTreeMap;

use saphyr_parser::{Event, Parser, ScalarStyle, Span, SpannedEventReceiver, Tag};
use serde_json::{Map, Number, Value};

use crate::{ConfigError, MAX_YAML_DEPTH, invalid};

pub(super) struct ParsedYaml(Node);

impl ParsedYaml {
    pub(super) fn into_json(self) -> Value {
        self.0.into_json()
    }
}

enum Node {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Sequence(Vec<Node>),
    Mapping(BTreeMap<String, Node>),
}

impl Node {
    fn into_json(self) -> Value {
        match self {
            Self::Null => Value::Null,
            Self::Bool(value) => Value::Bool(value),
            Self::Number(value) => Value::Number(value),
            Self::String(value) => Value::String(value),
            Self::Sequence(values) => {
                Value::Array(values.into_iter().map(Self::into_json).collect())
            }
            Self::Mapping(values) => Value::Object(
                values
                    .into_iter()
                    .map(|(key, value)| (key, value.into_json()))
                    .collect::<Map<_, _>>(),
            ),
        }
    }
}

struct Sink<'input> {
    events: Vec<(Event<'input>, Span)>,
}

impl<'input> SpannedEventReceiver<'input> for Sink<'input> {
    fn on_event(&mut self, event: Event<'input>, span: Span) {
        self.events.push((event, span));
    }
}

pub(super) fn parse(source: &str) -> Result<ParsedYaml, ConfigError> {
    let mut sink = Sink { events: Vec::new() };
    Parser::new_from_str(source)
        .load(&mut sink, true)
        .map_err(|error| invalid(format!("decode YAML: {error}")))?;
    let mut index = 0;
    expect(&sink.events, &mut index, |event| {
        matches!(event, Event::StreamStart)
    })?;
    let mut documents = Vec::new();
    while !matches!(event_at(&sink.events, index), Some(Event::StreamEnd)) {
        expect(&sink.events, &mut index, |event| {
            matches!(event, Event::DocumentStart(_))
        })?;
        documents.push(parse_node(&sink.events, &mut index, 0)?);
        expect(&sink.events, &mut index, |event| {
            matches!(event, Event::DocumentEnd)
        })?;
    }
    if documents.is_empty() {
        return Err(invalid("YAML document is empty"));
    }
    if documents.len() != 1 {
        return Err(invalid("multiple YAML documents are not supported"));
    }
    let document = documents
        .pop()
        .ok_or_else(|| invalid("YAML document is empty"))?;
    if matches!(document, Node::Null) {
        return Err(invalid("YAML document is empty"));
    }
    Ok(ParsedYaml(document))
}

fn parse_node(
    events: &[(Event<'_>, Span)],
    index: &mut usize,
    depth: usize,
) -> Result<Node, ConfigError> {
    if depth > MAX_YAML_DEPTH {
        return Err(at(
            events,
            *index,
            format!("YAML nesting exceeds {MAX_YAML_DEPTH} levels"),
        ));
    }
    let (event, span) = events
        .get(*index)
        .ok_or_else(|| invalid("decode YAML: unexpected end of event stream"))?;
    *index += 1;
    match event {
        Event::Alias(_) => Err(at_span(*span, "YAML anchors and aliases are not supported")),
        Event::Scalar(value, style, anchor, tag) => {
            validate_metadata(*anchor, tag.as_deref(), *span)?;
            scalar(value, *style, tag.as_deref(), *span)
        }
        Event::SequenceStart(anchor, tag) => {
            validate_metadata(*anchor, tag.as_deref(), *span)?;
            let mut values = Vec::new();
            while !matches!(event_at(events, *index), Some(Event::SequenceEnd)) {
                values.push(parse_node(events, index, depth + 1)?);
            }
            *index += 1;
            Ok(Node::Sequence(values))
        }
        Event::MappingStart(anchor, tag) => {
            validate_metadata(*anchor, tag.as_deref(), *span)?;
            let mut values = BTreeMap::new();
            while !matches!(event_at(events, *index), Some(Event::MappingEnd)) {
                let key_span = events.get(*index).map(|entry| entry.1).unwrap_or(*span);
                let key = match parse_node(events, index, depth + 1)? {
                    Node::String(key) => key,
                    _ => return Err(at_span(key_span, "YAML mapping keys must be strings")),
                };
                let value = parse_node(events, index, depth + 1)?;
                if values.insert(key.clone(), value).is_some() {
                    return Err(at_span(key_span, format!("duplicate YAML key {key:?}")));
                }
            }
            *index += 1;
            Ok(Node::Mapping(values))
        }
        _ => Err(at_span(*span, "decode YAML: expected a value")),
    }
}

fn validate_metadata(anchor: usize, tag: Option<&Tag>, span: Span) -> Result<(), ConfigError> {
    if anchor != 0 {
        return Err(at_span(span, "YAML anchors and aliases are not supported"));
    }
    if let Some(tag) = tag
        && !tag.is_yaml_core_schema()
    {
        return Err(at_span(
            span,
            format!("custom YAML tag {:?} is not supported", tag.to_string()),
        ));
    }
    Ok(())
}

fn scalar(
    value: &str,
    style: ScalarStyle,
    tag: Option<&Tag>,
    span: Span,
) -> Result<Node, ConfigError> {
    if style == ScalarStyle::Plain
        && matches!(
            value.to_ascii_lowercase().as_str(),
            "yes" | "no" | "on" | "off"
        )
    {
        return Err(at_span(
            span,
            format!("ambiguous YAML 1.1 value {value:?} must be quoted"),
        ));
    }
    if let Some(tag) = tag {
        return match tag.suffix.as_str() {
            "str" => Ok(Node::String(value.to_owned())),
            "null" => Ok(Node::Null),
            "bool" => parse_bool(value, span),
            "int" => parse_integer(value, span),
            "float" => parse_float(value, span),
            suffix => Err(at_span(
                span,
                format!("unsupported YAML core tag {suffix:?}"),
            )),
        };
    }
    if style != ScalarStyle::Plain {
        return Ok(Node::String(value.to_owned()));
    }
    let lower = value.to_ascii_lowercase();
    if matches!(lower.as_str(), "" | "~" | "null") {
        return Ok(Node::Null);
    }
    if matches!(lower.as_str(), "true" | "false") {
        return parse_bool(value, span);
    }
    if looks_like_integer(value) {
        return parse_integer(value, span);
    }
    if looks_like_float(value) {
        return parse_float(value, span);
    }
    Ok(Node::String(value.to_owned()))
}

fn parse_bool(value: &str, span: Span) -> Result<Node, ConfigError> {
    match value.to_ascii_lowercase().as_str() {
        "true" => Ok(Node::Bool(true)),
        "false" => Ok(Node::Bool(false)),
        _ => Err(at_span(span, format!("invalid YAML boolean {value:?}"))),
    }
}

fn looks_like_integer(value: &str) -> bool {
    let value = value.trim_start_matches(['+', '-']);
    value.starts_with("0x")
        || value.starts_with("0X")
        || value.starts_with("0o")
        || value.starts_with("0O")
        || (!value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte == b'_'))
}

fn parse_integer(value: &str, span: Span) -> Result<Node, ConfigError> {
    let cleaned = value.replace('_', "");
    let (negative, unsigned) = match cleaned.as_bytes().first() {
        Some(b'-') => (true, &cleaned[1..]),
        Some(b'+') => (false, &cleaned[1..]),
        _ => (false, cleaned.as_str()),
    };
    let (radix, digits) = if unsigned.starts_with("0x") || unsigned.starts_with("0X") {
        (16, &unsigned[2..])
    } else if unsigned.starts_with("0o") || unsigned.starts_with("0O") {
        (8, &unsigned[2..])
    } else {
        (10, unsigned)
    };
    let magnitude = u64::from_str_radix(digits, radix)
        .map_err(|_| at_span(span, format!("invalid YAML integer {value:?}")))?;
    if negative {
        let magnitude = i128::from(magnitude);
        let signed = i64::try_from(-magnitude)
            .map_err(|_| at_span(span, format!("YAML integer {value:?} is out of range")))?;
        Ok(Node::Number(Number::from(signed)))
    } else {
        Ok(Node::Number(Number::from(magnitude)))
    }
}

fn looks_like_float(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    if matches!(lower.as_str(), ".inf" | "+.inf" | "-.inf" | ".nan") {
        return true;
    }
    (lower.contains('.') || lower.contains('e')) && lower.replace('_', "").parse::<f64>().is_ok()
}

fn parse_float(value: &str, span: Span) -> Result<Node, ConfigError> {
    let cleaned = value.replace('_', "");
    let parsed: f64 = cleaned
        .parse()
        .map_err(|_| at_span(span, format!("invalid YAML number {value:?}")))?;
    let number =
        Number::from_f64(parsed).ok_or_else(|| at_span(span, "YAML number must be finite"))?;
    Ok(Node::Number(number))
}

fn event_at<'events, 'input>(
    events: &'events [(Event<'input>, Span)],
    index: usize,
) -> Option<&'events Event<'input>> {
    events.get(index).map(|entry| &entry.0)
}

fn expect(
    events: &[(Event<'_>, Span)],
    index: &mut usize,
    predicate: impl FnOnce(&Event<'_>) -> bool,
) -> Result<(), ConfigError> {
    let Some((event, span)) = events.get(*index) else {
        return Err(invalid("decode YAML: unexpected end of event stream"));
    };
    if !predicate(event) {
        return Err(at_span(*span, "decode YAML: unexpected parser event"));
    }
    *index += 1;
    Ok(())
}

fn at(events: &[(Event<'_>, Span)], index: usize, message: String) -> ConfigError {
    if let Some(entry) = events.get(index) {
        at_span(entry.1, message)
    } else {
        invalid(message)
    }
}

fn at_span(span: Span, message: impl Into<String>) -> ConfigError {
    invalid(format!(
        "line {}, column {}: {}",
        span.start.line(),
        span.start.col(),
        message.into()
    ))
}
