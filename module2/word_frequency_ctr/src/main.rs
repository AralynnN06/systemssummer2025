fn most_frequent_word(text: &str) -> (String, usize) {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut unique_words: Vec<&str> = Vec::new();
    let mut counts: Vec<usize> = Vec::new();

    for word in words {
        if let Some(pos) = unique_words.iter().position(|&w| w == word) {
            counts[pos] += 1;
        } else {
            unique_words.push(word);
            counts.push(1);
        }
    }

    let mut max_index = 0;
    for i in 1..counts.len() {
        if counts[i] > counts[max_index] {
            max_index = i;
        }
    }

    (unique_words[max_index].to_string(), counts[max_index])
}

fn main() {
    let text = "the quick brown fox jumps over the lazy dog the quick brown fox";
    let (word, count) = most_frequent_word(text);
    println!("Most frequent word: \"{}\" ({} times)", word, count);
}