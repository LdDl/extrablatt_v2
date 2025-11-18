use select::document::Document;
use select::predicate::{Name, Predicate};
use crate::Language;
use crate::text::{ArticleTextNode, ArticleTextNodeExtractor};

/// Detect the [`select::node::Node`] that contains the article's text.
///
/// If the `doc`'s body contains a node that matches the
/// [`crate::text::ARTICLE_BODY_ATTR`] attribute selectors, this node will
/// be selected. Otherwise the article node will be calculated by analysing
/// and scoring the textual content of text nodes.
pub fn article_node<'a>(doc: &'a Document, lang: Language) -> Option<ArticleTextNode<'a>> {
    let mut iter =
        doc.find(Name("body").descendant(ArticleTextNodeExtractor::article_body_predicate()));
    if let Some(node) = iter.next() {
        if iter.next().is_none() {
            return Some(ArticleTextNode::new(node));
        }
    }
    ArticleTextNodeExtractor::calculate_best_node(doc, lang)
}