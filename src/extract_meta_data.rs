use std::ops::Deref;
use select::document::Document;
use select::node::Node;
use select::predicate::{Name, Predicate};

/// Represents `<meta>` [`select::node::Node`] in a
/// [`select::document::Document`].
pub struct MetaNode<'a> {
    inner: Node<'a>,
}

impl<'a> MetaNode<'a> {
    pub fn attr<'b>(&'a self, attr: &'b str) -> Option<&'a str> {
        self.inner.attr(attr)
    }

    /// Value of the `name` attribute in the node.
    pub fn name_attr(&self) -> Option<&str> {
        self.attr("name")
    }

    /// Value of the `property` attribute in the node.
    pub fn property_attr(&self) -> Option<&str> {
        self.attr("property")
    }

    /// Value of the `content` attribute in the node.
    pub fn content_attr(&self) -> Option<&str> {
        self.attr("content")
    }

    /// Value of the `value` attribute in the node.
    pub fn value_attr(&self) -> Option<&str> {
        self.attr("value")
    }

    pub fn key(&self) -> Option<&str> {
        if let Some(prop) = self.property_attr() {
            Some(prop)
        } else {
            self.name_attr()
        }
    }

    pub fn value(&self) -> Option<&str> {
        if let Some(c) = self.content_attr() {
            Some(c)
        } else {
            self.value_attr()
        }
    }

    pub fn is_key_value(&self) -> bool {
        self.key().is_some() && self.value().is_some()
    }
}

impl<'a> Deref for MetaNode<'a> {
    type Target = Node<'a>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

/// Finds all `<meta>` nodes in the document.
pub fn meta_data<'a>(doc: &'a Document) -> Vec<MetaNode<'a>> {
    doc.find(Name("head").descendant(Name("meta")))
        .map(|node| MetaNode { inner: node })
        .filter(MetaNode::is_key_value)
        .collect()
}
