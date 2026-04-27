// index_text.rs - Extraction of indexable text from MIME parts

use mail_parser::{Message, PartType};

pub fn collect_text_parts(message: &Message) -> String {
    collect_parts_recursive(message, &mut (512 * 1024usize))
}

fn collect_parts_recursive(message: &Message, budget: &mut usize) -> String {
    let mut parts: Vec<String> = Vec::new();

    for part in message.text_bodies() {
        if *budget == 0 {
            break;
        }
        if let PartType::Text(text) = &part.body {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                let take = trimmed.len().min(*budget);
                parts.push(trimmed[..take].to_string());
                *budget = budget.saturating_sub(trimmed.len());
            }
        }
    }

    for part in message.attachments() {
        if *budget == 0 {
            break;
        }
        if let PartType::Message(inner) = &part.body {
            let inner_text = collect_parts_recursive(inner, budget);
            if !inner_text.is_empty() {
                parts.push(inner_text);
            }
        }
    }

    parts.join("\n\n")
}
