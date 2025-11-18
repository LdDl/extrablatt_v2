use std::collections::{HashMap, HashSet};
use std::ops::Deref;

use select::document::Document;
use select::node::Node;
use select::predicate::{Attr, Class, Name, Predicate};

use crate::clean::{DefaultDocumentCleaner, DocumentCleaner};
use crate::video::VideoNode;
use crate::Language;
use url::Url;

/// Expanded attribute key-value combinations to identify the root node for textual content
pub const ARTICLE_BODY_ATTR: &[(&str, &str)] = &[
    ("itemprop", "articleBody"),
    ("data-testid", "article-body"),
    ("name", "articleBody"),
    ("class", "content"),
    ("class", "article-content"),
    ("class", "post-content"),
    ("class", "entry-content"),
    ("class", "main-content"),
    ("id", "content"),
    ("id", "article-content"),
    ("id", "main-content"),
    ("role", "article"),
    ("data-role", "content"),
];

/// Negative attributes that indicate non-content sections
pub const NON_CONTENT_ATTR: &[(&str, &str)] = &[
    ("class", "sidebar"),
    ("class", "navigation"),
    ("class", "nav"),
    ("class", "menu"),
    ("class", "footer"),
    ("class", "header"),
    ("class", "advertisement"),
    ("class", "ad"),
    ("class", "comments"),
    ("class", "widget"),
    ("data-image-caption", ""), // Image caption marker
    ("role", "navigation"),
    ("role", "complementary"),
    ("data-role", "sidebar"),
];

pub const PUNCTUATION: &str = r###",."'!?&-/:;()#$%*+<=>@[\]^_`{|}~"###;

pub trait TextContainer<'a> {
    fn first_children_text(&self) -> Option<&'a str>;
    fn text_content_length(&self) -> usize;
    fn link_density(&self) -> f64;
    fn is_noise_node(&self) -> bool;
}

impl<'a> TextContainer<'a> for Node<'a> {
    fn first_children_text(&self) -> Option<&'a str> {
        self.children().find_map(|n| n.as_text())
    }

    fn text_content_length(&self) -> usize {
        self.text().chars().count()
    }

    fn link_density(&self) -> f64 {
        let link_text_length: usize = self.find(Name("a"))
            .map(|n| n.text().chars().count())
            .sum();
        
        let total_text_length = self.text_content_length();
        
        if total_text_length == 0 {
            return 1.0;
        }
        
        link_text_length as f64 / total_text_length as f64
    }

    fn is_noise_node(&self) -> bool {
        // Check for script, style, link, meta tags - these should NEVER be processed
        if Name("script").or(Name("style")).or(Name("link")).or(Name("meta")).or(Name("noscript")).or(Name("figcaption")).or(Name("figure")).matches(self) {
            return true;
        }

        // Check for image caption attributes
        if self.attr("data-image-caption").is_some() {
            return true;
        }

        // Check for ad containers (structural filtering)
        if self.attr("data-creative").is_some() {
            return true;
        }

        // Check for footer/bottom sections that often contain ads and related content
        if let Some(class) = self.attr("class") {
            let class_lower = class.to_lowercase();
            // Be specific to avoid false positives - check for clear footer/bottom patterns
            if class_lower.contains("articlebottom") ||
               class_lower.contains("article-bottom") ||
               class_lower.contains("footer") ||
               class_lower.contains("sidebar") ||
               class_lower.contains("widget") ||
               class_lower.contains("related-") ||
               class_lower.contains("recommendation") {
                return true;
            }
        }

        // Check for tracking pixels and invisible images
        if Name("img").matches(self) {
            if let Some(style) = self.attr("style") {
                if style.contains("display: none") ||
                   style.contains("visibility: hidden") ||
                   (style.contains("position: absolute") && style.contains("left: -9999px")) {
                    return true;
                }
            }
        }

        // Check if node is inside a script, style, figcaption, footer, or ad container tag
        let mut current = self.parent();
        while let Some(parent) = current {
            if Name("script").or(Name("style")).or(Name("noscript")).or(Name("figcaption")).or(Name("figure")).matches(&parent) {
                return true;
            }
            // Also check for data-image-caption attribute on parents
            if parent.attr("data-image-caption").is_some() {
                return true;
            }
            // Check if parent is an ad container (structural filtering)
            if parent.attr("data-creative").is_some() {
                return true;
            }
            // Check for footer/bottom sections in parent chain
            if let Some(class) = parent.attr("class") {
                let class_lower = class.to_lowercase();
                // Be specific to avoid false positives - check for clear footer/bottom patterns
                if class_lower.contains("articlebottom") ||
                   class_lower.contains("article-bottom") ||
                   class_lower.contains("footer") ||
                   class_lower.contains("sidebar") ||
                   class_lower.contains("widget") ||
                   class_lower.contains("related-") ||
                   class_lower.contains("recommendation") {
                    return true;
                }
            }
            current = parent.parent();
        }

        false
    }
}

pub struct TextNodeFind<'a> {
    document: &'a Document,
    next: usize,
}

impl<'a> TextNodeFind<'a> {
    fn is_text_node(node: &Node<'a>) -> bool {
        // Newspaper4k approach: be selective about divs
        if Name("p").or(Name("pre")).or(Name("td")).or(Name("article")).matches(node) {
            return true;
        }

        // For divs, only select those with article-related class/id
        if Name("div").matches(node) {
            // Check for article-related classes or IDs
            if let Some(class) = node.attr("class") {
                if class.contains("article") ||
                   class.contains("story") ||
                   class.contains("paragraph") ||
                   class.contains("content") ||
                   class.contains("post") ||
                   class.contains("entry") {
                    return true;
                }
            }
            if let Some(id) = node.attr("id") {
                if id.contains("article") ||
                   id.contains("story") ||
                   id.contains("content") {
                    return true;
                }
            }
            // Don't include generic divs
            return false;
        }

        false
    }

    fn is_bad(node: &Node<'a>) -> bool {
        // Expanded list of non-content elements
        Name("figure")
            .or(Name("figcaption")) // Filter out image captions
            .or(Name("media"))
            .or(Name("aside"))
            .or(Name("nav"))
            .or(Name("footer"))
            .or(Name("header"))
            .or(Name("script"))
            .or(Name("style"))
            .or(Name("link"))
            .or(Name("meta"))
            .or(Name("noscript"))
            .or(Class("advertisement").or(Class("ad")))
            .or(Class("sidebar"))
            .or(Class("navigation"))
            .or(Class("comments"))
            .or(Class("caption")) // Generic caption class
            .matches(node)
    }

    fn is_non_content_by_attr(node: &Node<'a>) -> bool {
        NON_CONTENT_ATTR.iter().any(|&(k, v)| Attr(k, v).matches(node))
    }

    fn new(document: &'a Document) -> Self {
        Self { document, next: 0 }
    }
}

impl<'a> Iterator for TextNodeFind<'a> {
    type Item = Node<'a>;

    fn next(&mut self) -> Option<Node<'a>> {
        while self.next < self.document.nodes.len() {
            let node = self.document.nth(self.next).unwrap();
            self.next += 1;
            
            if Self::is_bad(&node) || Self::is_non_content_by_attr(&node) || node.is_noise_node() {
                self.next += node.descendants().count();
                continue;
            }
            
            if Self::is_text_node(&node) {
                return Some(node);
            }
        }
        None
    }
}

#[derive(Debug, Clone)]
pub struct ArticleTextNode<'a> {
    inner: Node<'a>,
    confidence_score: f64,
}

impl<'a> ArticleTextNode<'a> {
    pub fn new(inner: Node<'a>) -> Self {
        Self {
            inner,
            confidence_score: 1.0,
        }
    }

    pub fn with_confidence(inner: Node<'a>, confidence_score: f64) -> Self {
        Self {
            inner,
            confidence_score,
        }
    }

    pub fn confidence_score(&self) -> f64 {
        self.confidence_score
    }

    /// Enhanced clean_text that aggressively filters out noise
    pub fn clean_text(&self) -> String {
        let raw_text = self.extract_clean_text();
        Self::post_process_text(&raw_text)
    }

    /// Extract text while filtering out noise nodes
    fn extract_clean_text(&self) -> String {
        let mut text_parts = Vec::new();

        // Newspaper4k-style: extract only from paragraph tags within the selected node
        for para in self.inner.find(Name("p")) {
            if para.is_noise_node() {
                continue;
            }

            // Structural filtering: skip promotional footer paragraphs
            if Self::is_promotional_footer(&para) {
                continue;
            }

            // Use .text() to get all text content from paragraph and its children
            let text = para.text();
            let trimmed = text.trim();
            if !trimmed.is_empty() && !Self::is_noise_text(trimmed) {
                text_parts.push(trimmed.to_string());
            }
        }

        text_parts.join(" ")
    }

    /// Check if a paragraph is promotional footer content based on link attributes.
    /// Promotional footers typically only contain links with rel="nofollow" attribute.
    fn is_promotional_footer(para: &Node) -> bool {
        let links: Vec<_> = para.find(Name("a")).collect();

        // If there are no links, it's not a promotional footer
        if links.is_empty() {
            return false;
        }

        // Check if ALL links have rel="nofollow" or rel="noopener nofollow"
        let all_nofollow = links.iter().all(|link| {
            if let Some(rel) = link.attr("rel") {
                rel.contains("nofollow")
            } else {
                false
            }
        });

        // If all links are nofollow, it's likely promotional content
        all_nofollow
    }
    
    /// Extract text while filtering out noise nodes
    fn extract_content_text(&self) -> String {
        let mut text_parts = Vec::new();
        
        // First, try to extract from proper paragraph structures
        let paragraphs: Vec<_> = self.inner.find(Name("p"))
            .filter(|n| !n.is_noise_node())
            .filter(|n| n.text_content_length() >= 20) // Minimum reasonable paragraph length
            .collect();
        
        if !paragraphs.is_empty() {
            for para in paragraphs {
                if let Some(text) = para.as_text() {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() && Self::is_likely_content_text(trimmed) {
                        text_parts.push(trimmed.to_string());
                    }
                }
            }
        }
        
        // If no good paragraphs found, fall back to general text extraction
        if text_parts.is_empty() {
            for node in self.inner.descendants() {
                if node.is_noise_node() {
                    continue;
                }
                
                // Skip nodes that are likely to contain code or metadata
                if Self::is_likely_code_node(&node) {
                    continue;
                }
                
                if let Some(text) = node.as_text() {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() && 
                       trimmed.len() >= 10 && 
                       Self::is_likely_content_text(trimmed) {
                        text_parts.push(trimmed.to_string());
                    }
                }
            }
        }
        
        text_parts.join("\n\n")
    }

    /// Check if text looks like meaningful content (not code, not metadata)
    fn is_likely_content_text(text: &str) -> bool {
        let text = text.trim();
        
        // Skip very short texts
        if text.len() < 10 {
            return false;
        }
        
        // Skip texts that contain JavaScript patterns
        if text.contains("function(") || 
           text.contains("=>") || 
           text.contains("const ") || 
           text.contains("let ") || 
           text.contains("var ") ||
           text.contains("getCookie") ||
           text.contains("navigator.") ||
           text.contains("XMLHttpRequest") {
            return false;
        }
        
        // Skip texts that are mostly URLs or file paths
        if text.starts_with("http://") || text.starts_with("https://") ||
           text.contains("/dist/") || text.contains("/assets/") ||
           text.contains(".css") || text.contains(".js") {
            return false;
        }
        
        // Skip texts that look like CSS or HTML attributes
        if (text.contains("class=") && text.contains("\"")) ||
           (text.contains("src=") && text.contains("\"")) ||
           (text.contains("alt=") && text.contains("\"")) ||
           (text.contains("height=") && text.contains("\"")) {
            return false;
        }
        
        // Skip tracking and analytics code
        if text.contains("webvisor:") || 
           text.contains("clickmap:") || 
           text.contains("trackLinks:") ||
           text.contains("analytics.") {
            return false;
        }
        
        // Check for reasonable text characteristics
        let alpha_count = text.chars().filter(|c| c.is_alphabetic()).count();
        let space_count = text.chars().filter(|c| c.is_whitespace()).count();
        let total_chars = text.chars().count();
        
        if total_chars == 0 {
            return false;
        }
        
        // Good content should have reasonable alphabetic and space ratios
        let alpha_ratio = alpha_count as f64 / total_chars as f64;
        let space_ratio = space_count as f64 / total_chars as f64;
        
        alpha_ratio > 0.4 && space_ratio > 0.05 && alpha_ratio < 0.95
    }

    /// Check if a node is likely to contain code rather than content
    fn is_likely_code_node(node: &Node) -> bool {
        // Script and style tags are already filtered by is_noise_node
        // Check for inline event handlers and other code indicators
        if let Some(attr) = node.attr("onclick")
            .or_else(|| node.attr("onload"))
            .or_else(|| node.attr("onsubmit")) {
            if attr.contains("function") || attr.contains("(") {
                return true;
            }
        }
        
        // Check parent nodes for code indicators
        if let Some(parent) = node.parent() {
            if parent.is(Name("script")) || parent.is(Name("style")) {
                return true;
            }
        }
        
        false
    }

    /// Post-process text to clean up formatting
    fn post_process_text(text: &str) -> String {
        let text = text.replace('\u{a0}', " ");
        let lines: Vec<&str> = text.lines().collect();
        let mut cleaned_lines = Vec::new();
        for line in lines {
            let trimmed = line.trim();
            
            if trimmed.is_empty() {
                continue;
            }
            
            // Skip lines that still look like code after initial filtering
            if Self::is_code_like_line(trimmed) {
                continue;
            }
            
            cleaned_lines.push(trimmed);
        }
        
        cleaned_lines.join("\n")
    }

     fn is_code_like_line(line: &str) -> bool {
        let line = line.trim();
        
        // Lines with lots of special characters but few words
        let word_count = line.split_whitespace().count();
        let special_char_count = line.chars().filter(|c| !c.is_alphanumeric() && !c.is_whitespace()).count();
        
        if word_count > 0 {
            let special_ratio = special_char_count as f64 / line.len() as f64;
            if special_ratio > 0.3 && word_count < 5 {
                return true;
            }
        }
        
        // Lines with specific code patterns
        line.contains(";") && line.contains("=") && !line.contains(" ") ||
        line.contains("()") && line.contains(".") ||
        line.starts_with("src=") || line.starts_with("alt=") ||
        line.starts_with("class=") || line.starts_with("height=")
    }

    /// Check if text looks like noise (CSS, scripts, etc.)
    fn is_noise_text(text: &str) -> bool {
        let text = text.trim();
        
        // Skip very short texts that are likely noise
        if text.len() < 10 {
            return false; // Keep short meaningful texts
        }
        
        // Skip CSS links and script content
        if text.contains("stylesheet") && text.contains(".css") ||
           text.contains("rel=\"") && text.contains("href=\"") ||
           text.contains("<script") || text.contains("<style") ||
           text.contains("<link") || text.contains("<meta") {
            return true;
        }
        
        // Skip tracking pixels and invisible content
        if text.contains("position: absolute") && text.contains("left: -9999px") {
            return true;
        }
        
        // Skip content that's mostly URLs or file paths
        if text.starts_with("http://") || text.starts_with("https://") ||
           text.contains("/dist/") || text.contains("/assets/") {
            return true;
        }
        
        // Skip content that looks like CSS classes or IDs
        if text.contains("-") && text.contains(".") && text.chars().any(|c| c.is_uppercase()) {
            let word_count = text.split_whitespace().count();
            if word_count == 1 && text.len() > 20 {
                return true;
            }
        }
        
        false
    }

    fn is_noise_line(line: &str) -> bool {
        let line = line.trim();
        
        // Skip lines that contain HTML tags
        if line.contains('<') && line.contains('>') {
            return true;
        }
        
        // Skip lines that are clearly CSS or JS
        if line.contains("{") && line.contains("}") || 
           line.contains(";") && line.contains(":") && !line.contains("://") ||
           line.starts_with("//") || line.starts_with("/*") {
            return true;
        }
        
        false
    }

    fn is_low_content_line(line: &str) -> bool {
        let alpha_count = line.chars().filter(|c| c.is_alphabetic()).count();
        let total_chars = line.chars().count();
        
        if total_chars == 0 {
            return true;
        }
        
        // If less than 30% of characters are alphabetic, it's probably not meaningful content
        (alpha_count as f64 / total_chars as f64) < 0.3
    }

    /// Extract all of the images of the document.
    pub fn images(&self, base_url: Option<&Url>) -> Vec<Url> {
        let options = Url::options().base_url(base_url);
        self.inner
            .find(Name("img"))
            .filter(|n| !n.is_noise_node())
            .filter_map(|n| n.attr("src").or_else(|| n.attr("data-src")).map(str::trim))
            .filter(|url| !url.is_empty())
            .filter_map(|url| options.parse(url).ok())
            .collect()
    }

    /// Extract all the links within the node's descendants
    pub fn references(&self) -> Vec<Url> {
        let mut uniques = HashSet::new();
        DefaultDocumentCleaner
            .iter_clean_nodes(self.inner)
            .filter(|n| Name("a").matches(n))
            .filter(|n| !n.is_noise_node())
            .filter_map(|n| n.attr("href").map(str::trim))
            .filter(|href| !href.is_empty())
            .filter(|href| uniques.insert(*href))
            .filter_map(|url| Url::parse(url).ok())
            .collect()
    }

    /// Extract all the nodes that hold video data
    pub fn videos(&self) -> Vec<VideoNode<'a>> {
        let mut videos: Vec<_> = self
            .inner
            .find(VideoNode::node_predicate())
            .filter(|n| !n.is_noise_node())
            .map(VideoNode::new)
            .collect();

        videos.extend(
            self.inner
                .find(Name("embed"))
                .filter(|n| !n.is_noise_node())
                .filter(|n| {
                    if let Some(parent) = n.parent() {
                        parent.name() != Some("object")
                    } else {
                        false
                    }
                })
                .map(VideoNode::new),
        );
        videos
    }
}

impl<'a> Deref for ArticleTextNode<'a> {
    type Target = Node<'a>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

pub struct ArticleTextNodeExtractor;

impl ArticleTextNodeExtractor {
    pub const MINIMUM_STOPWORD_COUNT: usize = 5;
    pub const MAX_STEPSAWAY_FROM_NODE: usize = 3;
    pub const MIN_TEXT_LENGTH: usize = 50;
    pub const MAX_LINK_DENSITY: f64 = 0.5;

    pub fn article_body_predicate() -> for<'r, 's> fn(&'r Node<'s>) -> bool {
        |node| {
            ARTICLE_BODY_ATTR.iter().any(|&(k, v)| Attr(k, v).matches(node))
        }
    }

    pub fn calculate_best_node(doc: &Document, lang: Language) -> Option<ArticleTextNode> {
        // Try to find explicit article body markers (only for itemprop="articleBody")
        if let Some(article_node) = doc.find(Attr("itemprop", "articleBody")).next() {
            return Some(ArticleTextNode::with_confidence(article_node, 0.95));
        }

        let mut starting_boost = 1.0;

        let txt_nodes: Vec<_> = ArticleTextNodeExtractor::nodes_to_check(doc)
            .filter(|n| !n.is_noise_node())
            .filter_map(|node| {
                // Extract text ONCE and reuse for all subsequent checks
                let text = node.text();
                let text_len = text.len();

                // Cheap checks first - fail fast before expensive operations
                // 1. Check length (cheapest - just len())
                if text_len < Self::MIN_TEXT_LENGTH {
                    return None;
                }

                // 2. Check if empty/noise (cheap string operations)
                if text.trim().is_empty() || ArticleTextNode::is_noise_text(&text) {
                    return None;
                }

                // 3. Check link density (medium cost - requires DOM traversal)
                let link_density = node.link_density();
                if link_density > Self::MAX_LINK_DENSITY {
                    return None;
                }

                // 4. Stopword counting LAST (most expensive operation!)
                if let Some(stats) = lang.stopword_count(&text) {
                    if stats.stopword_count >= Self::MINIMUM_STOPWORD_COUNT {
                        let score = Self::calculate_node_score(&node, stats.stopword_count);
                        return Some((node, stats, score));
                    }
                }
                None
            })
            .collect();

        let mut nodes_scores = HashMap::with_capacity(txt_nodes.len());
        let nodes_number = txt_nodes.len();
        let negative_scoring = 0.0;
        let bottom_negativescore_nodes = (nodes_number as f64 * 0.25).max(1.0);

        for (i, (node, stats, base_score)) in txt_nodes.iter().enumerate() {
            let mut boost_score = 0.0;

            if ArticleTextNodeExtractor::is_boostable(node, lang.clone()) {
                boost_score = (1.0 / starting_boost) * 50.0;
                starting_boost += 1.0;
            }

            if nodes_number > 15 {
                let score = (nodes_number - i) as f64;
                if score <= bottom_negativescore_nodes {
                    let booster = bottom_negativescore_nodes - score;
                    boost_score = booster.powf(2.0) * -1.0;

                    let negscore = boost_score.abs() + negative_scoring;
                    if negscore > 40.0 {
                        boost_score = 5.0;
                    }
                }
            }

            // Enhanced scoring with text length and formatting bonuses
            let formatting_bonus = Self::calculate_formatting_bonus(node);
            let length_bonus = (stats.word_count as f64 / 100.0).min(5.0);
            
            let upscore = (*base_score as f64 + boost_score + formatting_bonus + length_bonus) as usize;

            // Propagate score to parents with decay
            Self::propagate_score_to_parents(node, upscore, &mut nodes_scores);
        }

        // Find the best scoring node
        let (best_index, best_score) = nodes_scores
            .iter()
            .max_by_key(|(_, (score, _))| score)
            .map(|(&idx, &(score, _))| (idx, score))
            .unwrap_or((0, 0));

        // Calculate confidence based on score and other factors
        let confidence = Self::calculate_confidence(best_score, nodes_number);

        Some(ArticleTextNode::with_confidence(
            Node::new(doc, best_index).unwrap(),
            confidence,
        ))
    }

    fn calculate_node_score(node: &Node, stopword_count: usize) -> usize {
        let base_score = stopword_count;

        // Newspaper4k-style semantic HTML bonuses with high scores
        let mut semantic_bonus = 0;

        // Check for itemprop attributes (highest priority)
        if let Some(itemprop) = node.attr("itemprop") {
            semantic_bonus = match itemprop {
                "articleBody" => 100, // Newspaper4k gives this massive boost!
                "articleText" => 40,
                _ => 0,
            };
        }

        // Check for itemtype (Schema.org)
        if semantic_bonus == 0 {
            if let Some(itemtype) = node.attr("itemtype") {
                if itemtype.contains("schema.org/Article") || itemtype.contains("schema.org/NewsArticle") {
                    semantic_bonus = 30;
                } else if itemtype.contains("schema.org/BlogPosting") {
                    semantic_bonus = 20;
                }
            }
        }

        // Check for role attribute
        if semantic_bonus == 0 {
            if let Some(role) = node.attr("role") {
                if role == "article" {
                    if node.name() == Some("article") {
                        semantic_bonus = 25; // article + role="article"
                    } else {
                        semantic_bonus = 15;
                    }
                }
            }
        }

        // Fallback to tag-based bonuses
        if semantic_bonus == 0 {
            semantic_bonus = match node.name() {
                Some("article") => 10,
                Some("main") => 8,
                Some("section") => 5,
                Some("div") => 3,
                _ => 0,
            };
        }

        base_score + semantic_bonus
    }

    fn calculate_formatting_bonus(node: &Node) -> f64 {
        let mut bonus = 0.0;
        
        // Bonus for text formatting (indicates important content)
        if node.find(Name("strong")).next().is_some() {
            bonus += 2.0;
        }
        if node.find(Name("em")).next().is_some() {
            bonus += 1.5;
        }
        if node.find(Name("b")).next().is_some() {
            bonus += 1.0;
        }
        if node.find(Name("i")).next().is_some() {
            bonus += 0.5;
        }
        
        // Penalty for too many links
        let link_count = node.find(Name("a")).count();
        if link_count > 5 {
            bonus -= (link_count - 5) as f64 * 0.5;
        }
        
        bonus
    }

    fn propagate_score_to_parents(node: &Node, score: usize, nodes_scores: &mut HashMap<usize, (usize, usize)>) {
        // Newspaper4k approach: parent gets 100%, grandparent gets 40%
        // Parent node (100% of score)
        if let Some(parent) = node.parent() {
            let entry = nodes_scores.entry(parent.index()).or_insert((0, 0));
            entry.0 += score; // 100% of score to parent
            entry.1 += 1;

            // Grandparent node (40% of score)
            if let Some(grandparent) = parent.parent() {
                let grandparent_score = (score as f64 * 0.4) as usize;
                let entry = nodes_scores.entry(grandparent.index()).or_insert((0, 0));
                entry.0 += grandparent_score;
                entry.1 += 1;
            }
        }
    }

    fn calculate_confidence(score: usize, total_nodes: usize) -> f64 {
        if total_nodes == 0 {
            return 0.0;
        }
        
        let normalized_score = score as f64 / (total_nodes * 10) as f64;
        normalized_score.min(1.0).max(0.0)
    }

    /// Returns all nodes we want to search on like paragraphs and tables
    fn nodes_to_check(doc: &Document) -> impl Iterator<Item = Node> {
        TextNodeFind::new(doc)
    }

    /// Enhanced boostable check that considers both previous and next siblings
    fn is_boostable(node: &Node, lang: Language) -> bool {
        let mut steps_away = 0;
        
        // Check previous siblings
        let mut prev_sibling = node.prev();
        while let Some(sibling) = prev_sibling.filter(|n| n.is(Name("p"))) {
            if steps_away >= Self::MAX_STEPSAWAY_FROM_NODE {
                break;
            }
            if Self::is_quality_paragraph(&sibling, lang.clone()) {
                return true;
            }
            steps_away += 1;
            prev_sibling = sibling.prev();
        }
        
        // Check next siblings
        steps_away = 0;
        let mut next_sibling = node.next();
        while let Some(sibling) = next_sibling.filter(|n| n.is(Name("p"))) {
            if steps_away >= Self::MAX_STEPSAWAY_FROM_NODE {
                break;
            }
            if Self::is_quality_paragraph(&sibling, lang.clone()) {
                return true;
            }
            steps_away += 1;
            next_sibling = sibling.next();
        }
        
        false
    }

    fn is_quality_paragraph(node: &Node, lang: Language) -> bool {
        if node.link_density() > Self::MAX_LINK_DENSITY {
            return false;
        }
        
        if let Some(stats) = node
            .first_children_text()
            .and_then(|txt| lang.stopword_count(txt))
        {
            stats.stopword_count > Self::MINIMUM_STOPWORD_COUNT && 
            stats.word_count >= Self::MIN_TEXT_LENGTH / 5
        } else {
            false
        }
    }

    /// Returns an iterator over all words of the text.
    pub fn words(txt: &str) -> impl Iterator<Item = &str> {
        txt.split(|c: char| c.is_whitespace() || is_punctuation(c))
            .filter(|s| !s.is_empty())
    }
}

/// Whether the char is a punctuation.
pub fn is_punctuation(c: char) -> bool {
    PUNCTUATION.contains(c)
}

/// Enhanced author text extraction
pub fn author_text(node: Node) -> String {
    if Name("meta").matches(&node) {
        return node
            .attr("content")
            .map(str::to_string)
            .unwrap_or_else(String::new);
    }

    let mut string = String::new();
    let mut descendants = node.descendants();
    let hidden = Class("hidden").or(Class("sr-only")).or(Class("visually-hidden"));

    'outer: while let Some(child) = descendants.next() {
        if hidden.matches(&child) || child.is_noise_node() {
            // skip every node under this bad node
            for ignore in child.descendants() {
                if let Some(next) = descendants.next() {
                    if ignore.index() != next.index() {
                        continue 'outer;
                    }
                }
            }
        }
        if let Some(txt) = child.as_text() {
            if !ArticleTextNode::is_noise_text(txt) {
                string.push_str(txt);
                string.push(' '); // Add space between text nodes
            }
        }
    }
    
    // Clean up the extracted text
    string.trim().to_string()
}

/// Enhanced statistic about words for a text.
#[derive(Debug, Clone)]
pub struct WordsStats {
    /// All the words.
    pub word_count: usize,
    /// All the stop words.
    pub stopword_count: usize,
    /// Average word length
    pub avg_word_length: f64,
}