//! Embeddings locales por feature hashing (384-d) y puntuación BM25.
//!
//! El binario no descarga PyTorch. El hashing firmado es determinista entre
//! procesos (SHA-256, no el hasher aleatorio de Rust) y BM25 recupera las
//! guías por los términos de la pregunta. Si `CHROMA_URL` apunta a un servidor
//! Chroma, estos mismos vectores se envían por la API REST.

use std::collections::{HashMap, HashSet};

use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy)]
pub struct Embedder {
    pub dim: usize,
}

impl Embedder {
    pub fn new(dim: usize) -> Self {
        Self { dim: dim.max(8) }
    }

    pub fn embed(&self, text: &str) -> Vec<f32> {
        embed_hash(text, self.dim)
    }

    pub fn embed_many(&self, texts: &[String]) -> Vec<Vec<f32>> {
        texts.iter().map(|text| self.embed(text)).collect()
    }
}

pub fn tokenize(text: &str) -> Vec<String> {
    let folded = fold_accents(&text.to_lowercase());
    let mut tokens = Vec::new();
    for raw in folded.split(|ch: char| !ch.is_alphanumeric() && ch != '-') {
        let token = raw.trim_matches('-');
        if token.is_empty() {
            continue;
        }
        tokens.push(token.to_string());
        if token.contains('-') {
            for part in token.split('-') {
                if !part.is_empty() && part != token {
                    tokens.push(part.to_string());
                }
            }
        }
    }
    tokens
}

pub fn bm25_scores(query: &str, documents: &[String]) -> Vec<f32> {
    let query_tokens = tokenize(query);
    let doc_tokens: Vec<Vec<String>> = documents.iter().map(|doc| tokenize(doc)).collect();
    let n = doc_tokens.len() as f32;
    if n == 0.0 || query_tokens.is_empty() {
        return vec![0.0; documents.len()];
    }
    let avgdl = doc_tokens.iter().map(Vec::len).sum::<usize>() as f32 / n;
    let mut df: HashMap<String, usize> = HashMap::new();
    for doc in &doc_tokens {
        let mut seen = HashSet::new();
        for token in doc {
            if seen.insert(token.clone()) {
                *df.entry(token.clone()).or_insert(0) += 1;
            }
        }
    }
    let k1 = 1.5_f32;
    let b = 0.75_f32;
    let mut scores = Vec::with_capacity(doc_tokens.len());
    for doc in &doc_tokens {
        let dl = doc.len() as f32;
        let mut tf: HashMap<&str, usize> = HashMap::new();
        for token in doc {
            *tf.entry(token.as_str()).or_insert(0) += 1;
        }
        let mut score = 0.0_f32;
        for query_token in &query_tokens {
            let freq = *tf.get(query_token.as_str()).unwrap_or(&0) as f32;
            if freq == 0.0 {
                continue;
            }
            let doc_freq = *df.get(query_token).unwrap_or(&0) as f32;
            let idf = ((n - doc_freq + 0.5) / (doc_freq + 0.5) + 1.0).ln();
            let denom = freq + k1 * (1.0 - b + b * dl / avgdl.max(1.0));
            score += idf * (freq * (k1 + 1.0)) / denom;
        }
        scores.push(score);
    }
    scores
}

fn embed_hash(text: &str, dim: usize) -> Vec<f32> {
    let mut vector = vec![0.0_f32; dim];
    let tokens = tokenize(text);
    for token in &tokens {
        accumulate(&mut vector, token);
    }
    for pair in tokens.windows(2) {
        accumulate(&mut vector, &format!("{} {}", pair[0], pair[1]));
    }
    l2_normalize(&mut vector);
    vector
}

fn accumulate(vector: &mut [f32], feature: &str) {
    let hash = feature_hash(feature);
    let index = (hash as usize) % vector.len();
    let sign = if hash & 1 == 0 { 1.0 } else { -1.0 };
    vector[index] += sign;
}

fn feature_hash(token: &str) -> u64 {
    let digest = Sha256::digest(token.as_bytes());
    u64::from_le_bytes(digest[..8].try_into().expect("8 bytes"))
}

fn l2_normalize(vector: &mut [f32]) {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in vector.iter_mut() {
            *value /= norm;
        }
    }
}

fn fold_accents(text: &str) -> String {
    text.chars()
        .map(|ch| match ch {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            'Á' | 'À' | 'Ä' | 'Â' => 'a',
            'É' | 'È' | 'Ë' | 'Ê' => 'e',
            'Í' | 'Ì' | 'Ï' | 'Î' => 'i',
            'Ó' | 'Ò' | 'Ö' | 'Ô' => 'o',
            'Ú' | 'Ù' | 'Ü' | 'Û' => 'u',
            'Ñ' => 'n',
            other => other,
        })
        .collect()
}

pub fn dot(left: &[f32], right: &[f32]) -> f32 {
    left.iter().zip(right).map(|(a, b)| a * b).sum()
}

#[cfg(test)]
mod tests {
    use super::Embedder;

    #[test]
    fn embeddings_are_stable_and_distinct() {
        let embedder = Embedder::new(16);
        let vectors = embedder.embed_many(&["hola".to_string(), "mundo".to_string()]);
        assert_eq!(vectors.len(), 2);
        assert_eq!(vectors[0].len(), 16);
        assert_ne!(vectors[0], vectors[1]);
        assert_eq!(vectors[0], embedder.embed("hola"));
        let norm: f32 = vectors[0].iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
    }
}
