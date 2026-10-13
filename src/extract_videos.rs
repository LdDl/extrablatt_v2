use crate::Language;
use crate::extract_node::article_node;
use crate::video::VideoNode;
use select::document::Document;

/// All video content in the article.
pub fn videos<'a>(doc: &'a Document, lang: Option<Language>) -> Vec<VideoNode<'a>> {
    if let Some(node) = article_node(doc, lang.unwrap_or_default()) {
        node.videos()
    } else {
        Vec::new()
    }
}
