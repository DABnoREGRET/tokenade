//! **A question asked through a tokenade command, booked.** N questions
//! answered in one call are N−1 round-trips the agent did not make
//! (`record_batch_cache_saving`, label `questions`). `rag` answers and says
//! what it answered; booking is this layer's — the code layer never names the
//! product one.

/// [`crate::rag::prompt_scoped_extract_answers`], booked for `source`.
pub fn prompt_scoped_extract_asked(
    text: &str,
    prompt: &str,
    budget_tokens: usize,
    source: &str,
) -> Option<String> {
    let (out, answered) = crate::rag::prompt_scoped_extract_answers(text, prompt, budget_tokens);
    super::record_batch_cache_saving(source, "questions", &answered);
    out
}

/// [`crate::rag::answer_over_blob_answers`], booked for `source`.
pub fn answer_over_blob_asked(text: &str, prompt: &str, source: &str) -> Option<String> {
    let (out, answered) = crate::rag::answer_over_blob_answers(text, prompt);
    super::record_batch_cache_saving(source, "questions", &answered);
    out
}
