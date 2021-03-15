extern crate proc_macro;

use syn::{parse_macro_input, LitStr, LitByteStr};
use proc_macro::TokenStream;
use quote::ToTokens;

#[proc_macro]
pub fn rhexdump(input: TokenStream) -> TokenStream {
    let input_literal = parse_macro_input!(input as LitStr);
    let input = input_literal.value();

    let mut dehexed = Vec::new();

    for line in input.split("\n") {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let mut hexdump_parts = line.splitn(3, "   ");
        hexdump_parts.next().expect("badly formatted hex dump");
        let content = hexdump_parts.next().expect("badly formatted hex dump");

        let mut iter = HexByteIter(content.chars());
        dehexed.extend(&mut iter);
        iter.finalize();
    }

    LitByteStr::new(&dehexed, input_literal.span()).into_token_stream().into()
}

struct HexByteIter<'a>(std::str::Chars<'a>);

impl<'a> HexByteIter<'a> {
    fn finalize(&mut self) {
        for ch in &mut self.0 {
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
        while let Some(ch) = self.0.next() {
            if ch.is_whitespace() {
                continue;
            }
            if let Some(ny) = extract_nybble(ch) {
                bytes[byte_index] = ny;
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
        'A'..='F' => chn - CH_UPPER_A,
        'a'..='f' => chn - CH_LOWER_A,
        _ => return None,
    };
    assert!(nybble <= 0xF);
    Some(nybble as u8)
}

