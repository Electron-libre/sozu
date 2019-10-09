
use sozu_command::buffer::Buffer;
use buffer_queue::BufferQueue;

use nom::{HexDisplay,IResult,Offset,Err, character::complete::char, sequence::tuple};

use url::Url;

use std::str::{self, from_utf8};
use std::convert::From;
use std::collections::{HashMap,BTreeMap};
use std::borrow::Cow;

use super::{BufferMove, LengthInformation, RRequestLine, Connection, Chunk, Host, HeaderValue, TransferEncodingValue,
  Method, Version, Continue, Header, message_header, status_line, crlf, compare_no_case, sp, single_header_value};

#[derive(Debug,Clone,PartialEq,Eq,PartialOrd,Ord)]
struct BufferSlice {
  start: usize,
  length: usize,
}

impl BufferSlice {
  fn from_slice(s: &[u8]) -> Self {
    BufferSlice {
      start: s.as_ptr() as usize,
      length: s.len(),
    }
  }

  fn to_slice<'a, 'b>(&'a self) -> &'b[u8] {
    unsafe { std::slice::from_raw_parts(self.start as *const u8, self.length) }
  }
}

#[derive(Debug,Clone,PartialEq)]
enum ResponseParser {
  Initial,
  /// the usize indicates the current position
  ParsingHeaders(usize, StatusLineParser, BTreeMap<BufferSlice, HeaderValueParser>),
  HeadersParsed(usize, StatusLineParser, BTreeMap<BufferSlice, HeaderValueParser>),
  Error(usize, Option<StatusLineParser>, Option<BTreeMap<BufferSlice, HeaderValueParser>>),
}

#[derive(Debug,Clone,PartialEq)]
pub struct StatusLineParser {
  // maybe the span is not necessary: start at method start, end at version end+2
  span: BufferSlice,
  version: Version,
  status: BufferSlice,
  reason: BufferSlice,
}

#[derive(Debug,Clone,PartialEq)]
pub struct HeaderValueParser {
  // maybe the span is not necessary: start at header name start, end at header value end+2
  span: BufferSlice,
  value: BufferSlice,
}

impl ResponseParser {
  fn parse(self: ResponseParser, buffer: &[u8]) -> ResponseParser {
    match self {
      ResponseParser::Initial => match status_line(buffer) {
        Ok((i, r))    => {
          let offset = buffer.offset(i);
          let span = BufferSlice { start: buffer.as_ptr() as usize, length: offset };
          let status = BufferSlice::from_slice(r.status);
          let reason = BufferSlice::from_slice(r.reason);
          let version = r.version;

          let headers = BTreeMap::new();
          ResponseParser::ParsingHeaders(offset, StatusLineParser { span, status, reason, version }, headers)
        },
        Err(Err::Error(_)) | Err(Err::Failure(_)) => ResponseParser::Error(0, None, None),
        Err(Err::Incomplete(_)) => ResponseParser::Initial,
      },
      ResponseParser::ParsingHeaders(position, rl, mut headers) => match message_header(&buffer[position..]) {
        Ok((i, header)) => {
          let offset = (&buffer[position..]).offset(i);
          let span = BufferSlice { start: buffer.as_ptr() as usize, length: offset };
          let name = BufferSlice::from_slice(header.name);
          let value = BufferSlice::from_slice(header.value);

          let h = HeaderValueParser { span, value };

          //FIXME: we should accept multiple Set-Cookie headers
          if headers.contains_key(&name) {
            // we refuse duplicate headers
            ResponseParser::Error(position, Some(rl), Some(headers))
          } else {
            headers.insert(name, h);
            ResponseParser::ParsingHeaders(position + offset, rl, headers)
          }
        },
        Err(Err::Incomplete(_)) => ResponseParser::ParsingHeaders(position, rl, headers),
        Err(Err::Failure(_)) => ResponseParser::Error(position, Some(rl), Some(headers)),
        Err(Err::Error(_)) => match crlf(&buffer[position..]) {
          Ok((i, _)) => {
            // should be position + 2
            let offset = (&buffer[position..]).offset(i);
            ResponseParser::HeadersParsed(position + offset, rl, headers)
          },
          Err(Err::Incomplete(_)) => ResponseParser::ParsingHeaders(position, rl, headers),
          Err(Err::Error(_)) | Err(Err::Failure(_)) => ResponseParser::Error(position, Some(rl), Some(headers)),
        }
      },
      other => other,
    }
  }

  fn position(&self) -> usize {
    match self {
      ResponseParser::Initial => 0,
      ResponseParser::ParsingHeaders(p, _, _) |
      ResponseParser::HeadersParsed(p, _, _) |
      ResponseParser::Error(p, _, _) => *p
    }
  }

  fn is_error(&self) -> bool {
    match self {
      ResponseParser::Error(_, _, _) => true,
      _ => false,
    }
  }

  fn is_finished(&self) -> bool {
    match self {
      ResponseParser::HeadersParsed(_, _, _) |
      ResponseParser::Error(_, _, _) => true,
      _ => false,
    }
  }


  fn validate<'a, 'buffer>(&'a self, buffer: &'buffer[u8]) -> Option<ParsedResponse<'buffer>> {
    if let ResponseParser::HeadersParsed(ref position, ref rl, ref headers) = self {
      let status_line: ParsedStatusLine<'buffer> = ParsedStatusLine {
        span: rl.span.to_slice(),
        version: rl.version,
        reason: rl.reason.to_slice(),
        status: rl.status.to_slice(),
      };

      let mut parsed_headers: HashMap<ParsedHeaderName<'buffer>, ParsedHeaderValue<'buffer>> = HashMap::new();
      for (name, HeaderValueParser { span, value }) in headers.iter() {
        let name = name.to_slice();
        let span = span.to_slice();
        let value = value.to_slice();

        parsed_headers.insert(ParsedHeaderName::Ref(name), ParsedHeaderValue { span, value: Cow::from(value) });
      }

      Some(ParsedResponse {
        status_line,
        headers: parsed_headers,
        header_end: *position,
        connection: Connection::new(),
      })
    } else {
      None
    }
  }
}

#[derive(Debug,Clone,PartialEq)]
pub struct ParsedResponse<'a> {
  pub status_line: ParsedStatusLine<'a>,
  pub headers: HashMap<ParsedHeaderName<'a>, ParsedHeaderValue<'a>>,
  pub header_end: usize,
  pub connection: Connection,
}

impl<'a> ParsedResponse<'a> {
  pub fn status(&'a self) -> &'a[u8] {
    self.status_line.status
  }

  pub fn reason(&'a self) -> &'a[u8] {
    self.status_line.reason
  }

  pub fn version(&'a self) -> Version {
    self.status_line.version
  }

  pub fn host(&'a self) -> Option<&'a[u8]> {
    self.headers.get(&ParsedHeaderName::Ref(b"Host")).map(|v| v.as_slice())
  }

  pub fn connection(&'a self) -> &'a Connection {
    &self.connection
  }
  /*pub fn length_information(&'a self) -> Option<LengthInformation> {
    LengthInformation::Chunked 
    LengthInformation::Length(sz)
  }*/
}

#[derive(Debug,Clone,PartialEq)]
pub enum BodyLength {
  None,
  Length(usize),
  Chunked,
}

#[derive(Debug,Clone,PartialEq)]
pub struct ParsedStatusLine<'a> {
  span: &'a[u8],
  status: &'a[u8],
  reason: &'a[u8],
  version: Version,
}

#[derive(Debug,Clone,Eq,Ord)]
pub enum ParsedHeaderName<'a> {
  Ref(&'a[u8]),
  Allocated(Vec<u8>),
}

impl<'a> ParsedHeaderName<'a> {
  pub fn as_slice(&self) -> &[u8] {
    match self {
      ParsedHeaderName::Ref(s) => s,
      ParsedHeaderName::Allocated(v) => &v[..],
    }
  }
}

impl<'a> std::cmp::PartialEq for ParsedHeaderName<'a> {
  fn eq(&self, other: &Self) -> bool {
    compare_no_case(self.as_slice(), other.as_slice())
  }
}

impl<'a> std::hash::Hash for ParsedHeaderName<'a> {
  fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
    let sl = self.as_slice();
    sl.len().hash(state);

    for i in sl {
      match i {
        65..=90 | 97..=122 => (i | 0b00_10_00_00).hash(state),
        o => o.hash(state),
      }
    }
  }
}

impl<'a> std::cmp::PartialOrd for ParsedHeaderName<'a> {
  fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
    let s1 = self.as_slice();
    let s2 = other.as_slice();

    let l = std::cmp::min(s1.len(), s2.len());

    let lhs = &s1[..l];
    let rhs = &s2[..l];

    for i in 0..l {
      let res = match (lhs[i], rhs[i]) {
        (65..=90, 65..=90) | (97..=122, 97..=122) | (65..=90, 97..=122) | (97..=122, 65..=90) => (lhs[i] | 0b00_10_00_00).partial_cmp(&(rhs[i] | 0b00_10_00_00)),
        (a, b) => a.partial_cmp(&b)
      };
      match res {
        Some(std::cmp::Ordering::Equal) => (),
        non_eq => return non_eq,
      }
    }

    s1.len().partial_cmp(&s2.len())
  }
}

impl<'a> std::fmt::Display for ParsedHeaderName<'a> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "{}", std::str::from_utf8(self.as_slice()).unwrap())
  }
}

#[derive(Debug,Clone,PartialEq)]
pub struct ParsedHeaderValue<'a> {
  span: &'a[u8],
  value: Cow<'a, [u8]>,
}

impl<'a> ParsedHeaderValue<'a> {
  pub fn as_slice(&self) -> &[u8] {
    &self.value
  }

}
impl<'a> std::fmt::Display for ParsedHeaderValue<'a> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "{}", std::str::from_utf8(&self.value).unwrap())
  }
}

pub struct ValueIterator<'a> {
  data: &'a[u8],
  first: bool,
}

impl<'a> ValueIterator<'a> {
  pub fn new(data: &'a[u8]) -> Self {
    ValueIterator { data, first: true }
  }
}

impl<'a> Iterator for ValueIterator<'a> {
  type Item = &'a[u8];

  fn next(&mut self) -> Option<Self::Item> {
    if self.first {
      match single_header_value(self.data) {
        Ok((i, value)) => {
          self.data = i;
          self.first = false;
          Some(value)
        },
        _ => None,
      }
    } else {
      match tuple((sp, char(','), sp, single_header_value))(self.data) {
        Ok((i, (_, _, _, value))) => {
          self.data = i;
          Some(value)
        },
        _ => None,

      }
    }
  }
}

fn parse_and_validate(input: &[u8]) -> ParsedResponse {
  let mut state = ResponseParser::Initial;

  loop {
    let previous_position = state.position();

    state = state.parse(&input[..]);

    println!("state is now: {:?}", state);
    if state.is_error() {
      println!("got an error");
      break;
    }

    if state.position() == previous_position {
      println!("position did not change, failed advancing");
      break;
    }

    if state.is_finished() {
      println!("done");
      break;
    }
  }

  let req = state.validate(&input[..]).unwrap();

  println!("validated request: {:?}", req);
  for (name, value) in req.headers.iter() {
    println!("{} -> {}", name, value);
  }

  req
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn req() {
    let mut state = ResponseParser::Initial;
    let input =
        b"HTTP/1.1 200 OK\r\n\
          Content-Length: 200\r\n\
          \r\n";

    loop {
      let previous_position = state.position();

      state = state.parse(&input[..]);

      println!("state is now: {:?}", state);
      if state.is_error() {
        println!("got an error");
        break;
      }

      if state.position() == previous_position {
        println!("position did not change, failed advancing");
        break;
      }

      if state.is_finished() {
        println!("done");
        break;
      }
    }

    let req = state.validate(&input[..]).unwrap();
    println!("validated request: {:?}", req);
    for (name, value) in req.headers.iter() {
      println!("{} -> {}", name, value);
    }

    println!("requesting Host header: {}", req.headers.get(&ParsedHeaderName::Ref(&b"hOsT"[..])).unwrap());
    //panic!();
  }
}
