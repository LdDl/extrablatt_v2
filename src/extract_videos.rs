use select::document::Document;
use crate::Language;
use crate::video::VideoNode;
use crate::extract_node::article_node;

/// All video content in the article.
pub fn videos<'a>(doc: &'a Document, lang: Option<Language>) -> Vec<VideoNode<'a>> {
    if let Some(node) = article_node(doc, lang.unwrap_or_default()) {
        node.videos()
    } else {
        Vec::new()
    }
}
