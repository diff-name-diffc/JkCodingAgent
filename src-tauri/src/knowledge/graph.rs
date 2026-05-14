use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::Result;

use super::collection::find_collection;
use super::pages::list_pages_inner;
use super::types::{KnowledgeGraph, KnowledgeGraphEdge, KnowledgeGraphNode};
use super::utils::{page_slug, slugify, spawn_blocking_string};

fn extract_wikilinks(content: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut rest = content;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else {
            break;
        };
        let target = after[..end].split('|').next().unwrap_or("").trim();
        if !target.is_empty() {
            links.push(target.to_string());
        }
        rest = &after[end + 2..];
    }
    links
}

fn add_edge_weight(
    edges: &mut BTreeMap<(String, String), (f32, String)>,
    pair: (String, String),
    weight: f32,
    reason: &str,
) {
    let entry = edges.entry(pair).or_insert((0.0, reason.to_string()));
    entry.0 += weight;
    if !entry.1.contains(reason) {
        entry.1.push_str(", ");
        entry.1.push_str(reason);
    }
}

fn ordered_pair(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

fn extract_frontmatter_array(content: &str, key: &str) -> BTreeSet<String> {
    super::pages::extract_frontmatter_array(content, key)
}

fn normalize_source_name(name: String) -> String {
    std::path::Path::new(&name.replace("\\\"", "\"").replace("\\\\", "\\"))
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&name.replace("\\\"", "\"").replace("\\\\", "\\"))
        .to_string()
}

fn build_graph_inner(collection: &super::types::KnowledgeCollection) -> Result<KnowledgeGraph> {
    let pages = list_pages_inner(collection)?;
    let mut slug_to_path = HashMap::new();
    let mut path_to_title = HashMap::new();
    let mut path_to_type = HashMap::new();
    let mut path_to_sources = HashMap::new();
    let mut path_to_links = HashMap::new();

    for page in &pages {
        let content = std::fs::read_to_string(&page.path).unwrap_or_default();
        let slug = page_slug(&page.path);
        slug_to_path.insert(slug.clone(), page.path.clone());
        slug_to_path.insert(slugify(&page.title), page.path.clone());
        path_to_title.insert(page.path.clone(), page.title.clone());
        path_to_type.insert(page.path.clone(), page.page_type.clone());
        path_to_sources.insert(
            page.path.clone(),
            extract_frontmatter_array(&content, "sources")
                .into_iter()
                .map(normalize_source_name)
                .collect::<HashSet<_>>(),
        );
        path_to_links.insert(page.path.clone(), extract_wikilinks(&content));
    }

    let nodes = pages
        .iter()
        .map(|page| KnowledgeGraphNode {
            id: page.path.clone(),
            label: page.title.clone(),
            page_type: page.page_type.clone(),
            path: page.relative_path.clone(),
        })
        .collect::<Vec<_>>();

    let mut edge_weights: BTreeMap<(String, String), (f32, String)> = BTreeMap::new();
    for (source_path, links) in &path_to_links {
        for link in links {
            let key = slugify(link);
            if let Some(target_path) = slug_to_path.get(&key) {
                if target_path != source_path {
                    let pair = ordered_pair(source_path, target_path);
                    add_edge_weight(&mut edge_weights, pair, 3.0, "wikilink");
                }
            }
        }
    }

    // Inverted-index approach: O(n*s) instead of O(n²*s) for source-overlap edges
    let mut source_to_pages: HashMap<String, Vec<&String>> = HashMap::new();
    for (page_path, sources) in &path_to_sources {
        for source in sources {
            source_to_pages
                .entry(source.clone())
                .or_insert_with(|| Vec::new())
                .push(page_path);
        }
    }
    let mut seen_pairs: HashSet<(String, String)> = HashSet::new();
    for pages_sharing_source in source_to_pages.values() {
        if pages_sharing_source.len() < 2 {
            continue;
        }
        for i in 0..pages_sharing_source.len() {
            for j in (i + 1)..pages_sharing_source.len() {
                let pair = ordered_pair(pages_sharing_source[i], pages_sharing_source[j]);
                if seen_pairs.insert(pair.clone()) {
                    let a_sources = path_to_sources
                        .get(pages_sharing_source[i])
                        .cloned()
                        .unwrap_or_default();
                    let b_sources = path_to_sources
                        .get(pages_sharing_source[j])
                        .cloned()
                        .unwrap_or_default();
                    let overlap = a_sources.intersection(&b_sources).count();
                    if overlap > 0 {
                        add_edge_weight(
                            &mut edge_weights,
                            pair,
                            1.0 + overlap as f32,
                            "source-overlap",
                        );
                    }
                }
            }
        }
    }

    let edges = edge_weights
        .into_iter()
        .map(|((source, target), (weight, reason))| KnowledgeGraphEdge {
            source,
            target,
            weight,
            reason,
        })
        .collect();
    Ok(KnowledgeGraph { nodes, edges })
}

#[tauri::command]
pub async fn knowledge_build_graph(collection_id: String) -> Result<KnowledgeGraph, String> {
    spawn_blocking_string(move || {
        let collection = find_collection(&collection_id)?;
        build_graph_inner(&collection)
    })
    .await
}
