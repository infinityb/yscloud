extern crate proc_macro;

use std::error::Error;

use syn::{parse_macro_input, LitStr, LitByteStr};
use proc_macro::TokenStream;
use quote::ToTokens;

fn string_error(message: &str) -> Box<Error> {
    message.into()
}

fn split2<'a>(data: &'a str, sep: &str) -> Option<(&'a str, &'a str)> {
    let mut splitting = data.splitn(2, sep);
    let first = splitting.next()?;
    let second = splitting.next()?;
    Some((first, second))
}

type DecoderFn = for<'a> fn(&'a str) -> std::result::Result<HexByteIter<'a>, Box<Error>>;

fn xxd_decode_line<'a>(line: &'a str) -> Result<HexByteIter<'a>, Box<Error>> {
    let (_offset, rest) = split2(line, ": ")
        .ok_or_else(|| string_error("no body found after offset"))?;
    
    let hexdata = rest.splitn(2, "  ").next().unwrap();
    Ok(HexByteIter { chars: hexdata.chars() })
}

fn other_decode_line<'a>(line: &'a str) -> Result<HexByteIter<'a>, Box<Error>> {
    let mut hexdump_parts = line.splitn(3, "   ");
    let _offset = hexdump_parts.next()
        .ok_or_else(|| string_error("no body found after offset"))?;
    let content = hexdump_parts.next()
        .ok_or_else(|| string_error("badly formatted hex dump"))?;
    Ok(HexByteIter { chars: content.chars() })
}

fn determine_decoder(first_line: &str) -> Result<DecoderFn, Box<Error>> {
    let mut decoder: Option<DecoderFn> = None;
    if let Ok(..) = other_decode_line(&first_line) {
        decoder = Some(other_decode_line);
    }
    if let Ok(..) = xxd_decode_line(&first_line) {
        decoder = Some(xxd_decode_line);
    }
    decoder.ok_or_else(|| string_error("No valid decoders could be found."))
}

#[proc_macro]
pub fn rhexdump(input: TokenStream) -> TokenStream {
    let input_literal = parse_macro_input!(input as LitStr);
    let input = input_literal.value();
    let dehexed = rhexdump_impl(&input).unwrap();
    LitByteStr::new(&dehexed, input_literal.span()).into_token_stream().into()
}

fn rhexdump_impl(input: &str) -> Result<Vec<u8>, Box<Error>> {
    let mut dehexed = Vec::new();

    let mut line_iter = input.trim().split("\n");

    let mut decision_line_iter = line_iter.clone();
    let first_line = decision_line_iter.next().expect("there must be a first line");
    let decoder = determine_decoder(&first_line)?;

    for line in line_iter {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let mut iter = decoder(line)?;
        dehexed.extend(&mut iter);
        iter.finalize();
    }

    Ok(dehexed)
}

struct HexByteIter<'a> {
    chars: std::str::Chars<'a>,
}

impl<'a> HexByteIter<'a> {
    fn finalize(&mut self) {
        for ch in &mut self.chars {
            if !ch.is_whitespace() {
                panic!("non-whitespace character detected after consumption");
            }
        }
    }
}

impl<'a> Iterator for &mut HexByteIter<'a> {
    type Item = u8;

    fn next(&mut self) -> Option<Self::Item> {
        let mut byte_index = 0;
        let mut bytes: [u8; 2] = [0; 2];
        while let Some(ch) = self.chars.next() {
            if ch.is_whitespace() {
                continue;
            }
            if let Some(ny) = extract_nybble(ch) {
                bytes[byte_index] = ny;
                eprintln!("consume {:?}[{:?}] = {:?}", bytes, byte_index, ch);
                byte_index += 1;
            } else {
                panic!("character {} is neither whitespace nor hex", ch);
            }
            if byte_index == 2 {
                let mut acc = bytes[0] << 4;
                acc += bytes[1];
                return Some(acc);
            }
        }
        if byte_index != 0 {
            panic!("odd-length hex");
        }
        None
    } 
}

fn extract_nybble(ch: char) -> Option<u8> {
    const CH_ZERO: u32 = '0' as u32;
    const CH_LOWER_A: u32 = 'a' as u32;
    const CH_UPPER_A: u32 = 'A' as u32;

    let chn = ch as u32;
    let nybble = match ch {
        '0'..='9' => chn - CH_ZERO,
        'A'..='F' => 10 + chn - CH_UPPER_A,
        'a'..='f' => 10 + chn - CH_LOWER_A,
        _ => return None,
    };
    assert!(nybble <= 0xF);
    Some(nybble as u8)
}

#[test]
fn sample_dump1() {
    assert_eq!(rhexdump_impl(r#"
        0000   02 01 06 00 c7 ea fb ae 00 00 00 00 4b 9f 2e 3f   ............K..?
        0010   4b 9f 2e 3f 0a 98 d3 0d 00 00 00 00 00 50 04 b1   K..?.........P..
    "#).unwrap(), &[
        0x02, 0x01, 0x06, 0x00, 0xc7, 0xea, 0xfb, 0xae, 0x00, 0x00, 0x00, 0x00,
        0x4b, 0x9f, 0x2e, 0x3f, 0x4b, 0x9f, 0x2e, 0x3f, 0x0a, 0x98, 0xd3, 0x0d,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x50, 0x04, 0xb1,
    ]);
}


#[test]
fn sample_dump_xxd_single_line() {
    assert_eq!(rhexdump_impl(r#"
        00000000: 6173 6466 0a                             asdf.
    "#).unwrap(), &[0x61, 0x73, 0x64, 0x66, 0x0a]);
}

#[test]
fn sample_dump_xxd_multiple_line() {
    assert_eq!(rhexdump_impl(r#"
        00000000: 6173 6466 6173 6466 6173 6466 6173 6466  asdfasdfasdfasdf
        00000010: 6173 6466 0a                             asdf.
    "#).unwrap(), &[
        0x61, 0x73, 0x64, 0x66, 0x61, 0x73, 0x64, 0x66, 0x61, 0x73, 0x64,0x66,
        0x61, 0x73, 0x64, 0x66, 0x61, 0x73, 0x64, 0x66, 0x0a
    ]);
}

