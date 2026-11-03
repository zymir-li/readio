//! Lossless sentence-sized prose units; original book files are never modified.
use crate::book::{Book, Emphasis, Para, Rich};

pub fn prepare(book: &mut Book) {
    for chapter in &mut book.chapters {
        chapter.paras = std::mem::take(&mut chapter.paras)
            .into_iter()
            .flat_map(|para| {
                let quote = matches!(&para, Para::Quote(_));
                let rich = match para {
                    Para::Text(r) | Para::Quote(r) => r,
                    other => return vec![other],
                };
                if rich.text.chars().count() <= 120 {
                    return vec![if quote {
                        Para::Quote(rich)
                    } else {
                        Para::Text(rich)
                    }];
                }
                let mut ranges = Vec::new();
                let mut start = 0;
                for sentence in crate::voice::sentence::sentences(&rich.text) {
                    let end = sentence.range.1;
                    if end > start && rich.text[start..end].chars().count() >= 70 {
                        ranges.push((start, end));
                        start = end;
                    }
                }
                if start < rich.text.len() {
                    if rich.text[start..].trim().is_empty() && !ranges.is_empty() {
                        ranges.last_mut().unwrap().1 = rich.text.len();
                    } else {
                        ranges.push((start, rich.text.len()));
                    }
                }
                ranges
                    .into_iter()
                    .map(|(start, end)| {
                        let emphasis = rich
                            .emphasis
                            .iter()
                            .filter_map(|span| {
                                let lo = (span.start as usize).max(start);
                                let hi = (span.end as usize).min(end);
                                (lo < hi).then(|| Emphasis {
                                    start: (lo - start) as u32,
                                    end: (hi - start) as u32,
                                    strong: span.strong,
                                })
                            })
                            .collect();
                        let r = Rich {
                            text: rich.text[start..end].to_string(),
                            emphasis,
                        };
                        if quote { Para::Quote(r) } else { Para::Text(r) }
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sentence_units_keep_every_character_and_resume_coordinate() {
        let mut book = Book::load(None).unwrap();
        let text = "完整的句子必须原样保存，不能改写或者漏掉内容。  ".repeat(18);
        book.chapters.truncate(1);
        book.chapters[0].paras = vec![Para::Text(text.clone().into()), Para::Text("下一段".into())];
        let old_boundary = text.chars().count();
        prepare(&mut book);
        let joined: String = book.chapters[0].paras.iter().map(Para::text).collect();
        assert_eq!(joined, format!("{text}下一段"));
        assert!(book.chapters[0].paras.len() > 2);
        let (ch, p) = book.locate(old_boundary);
        assert_eq!(book.chars_before(ch, p), old_boundary);
        assert_eq!(book.chapters[ch].paras[p].text(), "下一段");
        let once = book.chapters[0].paras.clone();
        prepare(&mut book);
        assert_eq!(book.chapters[0].paras, once);
    }
    #[test]
    fn emphasis_and_code_are_preserved() {
        let mut book = Book::load(None).unwrap();
        let text = "Sentence with emphasized content. ".repeat(15);
        let rich = Rich {
            text: text.clone(),
            emphasis: vec![Emphasis {
                start: 0,
                end: text.len() as u32,
                strong: true,
            }],
        };
        book.chapters.truncate(1);
        book.chapters[0].paras = vec![Para::Quote(rich), Para::Code("println!(42);".into())];
        prepare(&mut book);
        for p in &book.chapters[0].paras[..book.chapters[0].paras.len() - 1] {
            let Para::Quote(r) = p else {
                panic!("quote lost")
            };
            assert_eq!(r.emphasis[0].end as usize, r.text.len());
        }
        assert_eq!(
            book.chapters[0].paras.last(),
            Some(&Para::Code("println!(42);".into()))
        );
    }
}
