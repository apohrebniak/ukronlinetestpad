use core::str;
use regex::Regex;
use std::io::{BufReader, Cursor, Read, Seek, Write};
use wasm_bindgen::prelude::*;
use web_sys::{
    js_sys::Array, js_sys::Uint8Array, window, Blob, Event, File, FileList, FileReader,
    HtmlAnchorElement, HtmlInputElement, Url,
};
use zip::write::SimpleFileOptions;
use zip::ZipArchive;

#[wasm_bindgen]
pub fn handle_event(event: &Event) {
    // let event: &InputEvent = event.dyn_ref().unwrap();
    let file_input: HtmlInputElement = event.target().unwrap().dyn_into().unwrap();
    let file_list: FileList = file_input.files().unwrap();
    let file: File = file_list.item(0).unwrap();
    let file_name = file.name();
    let blob: Blob = file.slice().unwrap();

    let file_reader = FileReader::new().unwrap();
    file_reader.read_as_array_buffer(&blob).unwrap();

    if file_reader.result().is_err() {
        alert_error();
        return;
    };

    let file_reader_clone = file_reader.clone();
    let onloadend_closure = Closure::wrap(Box::new(move || {
        if file_reader_clone.ready_state() == FileReader::DONE {
            let array_buffer = file_reader_clone.result().unwrap();
            let src_bytes = Uint8Array::new(&array_buffer).to_vec();

            let Ok(out_bytes) = process_bytes(src_bytes) else {
                alert_error();
                return;
            };

            download_file(out_bytes, file_name.as_str());
        }
    }) as Box<dyn Fn()>);
    file_reader.set_onloadend(Some(onloadend_closure.as_ref().unchecked_ref()));
    onloadend_closure.forget();
}

fn download_file(bytes: Vec<u8>, name: &str) {
    let array = Uint8Array::new_with_length(bytes.len() as u32);
    array.copy_from(&bytes[..]);

    let parts = Array::new();
    parts.push(&array.buffer());
    let blob = Blob::new_with_u8_array_sequence(&parts).unwrap();

    let window = web_sys::window().unwrap();
    let document = window.document().unwrap();
    let url = Url::create_object_url_with_blob(&blob).unwrap();

    let link: HtmlAnchorElement = document.create_element("a").unwrap().dyn_into().unwrap();
    link.set_href(&url);
    link.set_download(format!("укр_{}", name).as_str());
    link.click();
    // Clean up by revoking the object URL
    Url::revoke_object_url(&url).unwrap();
}

fn alert_error() {
    window().unwrap().alert_with_message("Помилка!").unwrap()
}

struct Error;

fn process_bytes(src_bytes: Vec<u8>) -> Result<Vec<u8>, Error> {
    let src_bytes = Cursor::new(src_bytes);
    let src_zip_reader = BufReader::new(src_bytes);

    let mut src_zip = zip::ZipArchive::new(src_zip_reader).map_err(|_| Error)?;

    let Some(target_file_name) = find_target_file(src_zip.file_names()) else {
        return Err(Error); // no file to alter. this is the error
    };
    let target_file_name = target_file_name.to_string();

    let mut target_file_bytes = Vec::new();
    {
        let mut target_file = src_zip
            .by_name(target_file_name.as_str())
            .map_err(|_| Error)?;
        target_file
            .read_to_end(&mut target_file_bytes)
            .map_err(|_| Error)?;
    }

    // perform all the modifications
    let target_file_bytes = modify_file(target_file_bytes)?;

    let target_bytes = build_new_archive(
        src_zip,
        target_file_name.as_str(),
        target_file_bytes.as_slice(),
    )?;

    Ok(target_bytes)
}

fn build_new_archive<T: Read + Seek>(
    mut src_zip: ZipArchive<T>,
    target_file_name: &str,
    target_file_bytes: &[u8],
) -> Result<Vec<u8>, Error> {
    let mut zip_writer = zip::ZipWriter::new(Cursor::new(Vec::new()));

    let src_zip_file_names: Vec<String> = src_zip.file_names().map(|s| s.to_string()).collect();

    // copy contents
    for name_in_zip in src_zip_file_names.iter() {
        // do not copy modified file ofc
        if name_in_zip.as_str().eq(target_file_name) {
            continue;
        }

        let Ok(file_in_zip) = src_zip.by_name(name_in_zip) else {
            return Err(Error);
        };

        zip_writer.raw_copy_file(file_in_zip).map_err(|_| Error)?;
    }

    zip_writer
        .start_file(target_file_name, SimpleFileOptions::default())
        .map_err(|_| Error)?;
    zip_writer.write_all(target_file_bytes).map_err(|_| Error)?;
    let result_zip_file = zip_writer.finish().map_err(|_| Error)?;

    Ok(result_zip_file.into_inner())
}

fn find_target_file<'a>(mut names: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let regex = Regex::new(r"main.(.*).bundle.js").unwrap();
    names.find(|name| regex.is_match(name))
}

fn modify_file(bytes: Vec<u8>) -> Result<Vec<u8>, Error> {
    let obfuscated = String::from_utf8(bytes).map_err(|_| Error)?;
    let mut chars = obfuscated.chars();

    let mut unescaped = String::new();

    while let Some(c) = chars.next() {
        // safe char
        if c != '\\' {
            unescaped.push(c);
            continue;
        }

        let Some(next_after_backslash) = chars.next() else {
            unescaped.push('\\');
            break;
        };

        // not a start of unicode sequence
        if next_after_backslash != 'u' {
            unescaped.push('\\');
            unescaped.push(next_after_backslash);
            continue;
        }

        let hexnum = vec![
            chars.next().unwrap() as u8,
            chars.next().unwrap() as u8,
            chars.next().unwrap() as u8,
            chars.next().unwrap() as u8,
        ];

        let encoded_utf16_char =
            u16::from_str_radix(unsafe { str::from_utf8_unchecked(hexnum.as_slice()) }, 16)
                .map_err(|_| Error)?;

        let decoded_char = char::decode_utf16([encoded_utf16_char])
            .next()
            .ok_or(Error)?
            .map_err(|_| Error)?;

        unescaped.push(decoded_char);
    }

    let ac = aho_corasick::AhoCorasick::new(PATTERNS).map_err(|_| Error)?;
    let replaced = ac.replace_all(unescaped.as_str(), &REPLACEMENTS);

    Ok(replaced.into_bytes())
}

#[rustfmt::skip]
const PATTERNS: [&str; 15] = [
    // Phrases
    "Далее",
    "Завершить",
    "Количество вопросов в тесте",
    "Ответьте, пожалуйста, на вопрос",
    "Показать мои ответы",
    "Показать мой результат",
    "Показатель",
    "Значение",
    "Количество баллов (правильных ответов)",
    "Максимально возможное количество баллов",
    "Процент",
    "из",

    // Links
    "Powered by",
    "Online Test Pad",
    "http://onlinetestpad.com",
];

#[rustfmt::skip]
const REPLACEMENTS: [&str; 15] = [
    // Phrases
    "Далі",
    "Закінчити",
    "Кількість питань в тесті",
    "Дайте відповідь на запитання",
    "Мої відповіді",
    "Mій результат",
    "Показник",
    "Значення",
    "Кількість балів (правильних відповідей)",
    "Максимальна кількість балів",
    "Відсоток",
    "з",

    // Links
    "",
    "",
    "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
];
