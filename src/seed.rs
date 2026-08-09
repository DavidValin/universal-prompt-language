// seed.rs
//
// Bundles the prompt samples shipped in the repo's `samples/` folder directly
// into the compiled binary, so that on first run (when `~/.upl` does not exist
// yet) the user gets a ready-to-use prompt library and its `tags_db` placed at
// `~/.upl/prompts/` and `~/.upl/tags_db` respectively.
//
// The embedded `tags_db` is the bincode-serialized `TagStore` shipped in
// `samples/tags_db`; its sha256 associations match the embedded prompt files
// byte-for-byte (the hashes were computed over the exact same file contents).

use std::fs;
use std::io;

use crate::repository::protocol::upl_home;

// Each sample prompt is embedded as a &str. The base file name is preserved
// when writing it to `~/.upl/prompts/`.
const SAMPLES: &[(&str, &str)] = &[
    ("analyze_argument.txt", include_str!("../samples/analyze_argument.txt")),
    ("analyze_psychological_facts.txt", include_str!("../samples/analyze_psychological_facts.txt")),
    ("brainstorm_ideas.txt", include_str!("../samples/brainstorm_ideas.txt")),
    ("cognitive_behavioral_exercise.txt", include_str!("../samples/cognitive_behavioral_exercise.txt")),
    ("career_development_plan.txt", include_str!("../samples/career_development_plan.txt")),
    ("create_a_plan.txt", include_str!("../samples/create_a_plan.txt")),
    ("create_rest_api.txt", include_str!("../samples/create_rest_api.txt")),
    ("explain_subject.txt", include_str!("../samples/explain_subject.txt")),
    ("financial_plan.txt", include_str!("../samples/financial_plan.txt")),
    ("improve_personal_finances.txt", include_str!("../samples/improve_personal_finances.txt")),
    ("implement_user_story.txt", include_str!("../samples/implement_user_story.txt")),
    ("marketing_campaign.txt", include_str!("../samples/marketing_campaign.txt")),
    ("monthly_food_plan.txt", include_str!("../samples/monthly_food_plan.txt")),
    ("review_article.txt", include_str!("../samples/review_article.txt")),
    ("review_contract.txt", include_str!("../samples/review_contract.txt")),
    ("teach_foundations.txt", include_str!("../samples/teach_foundations.txt")),
    ("wellness_plan.txt", include_str!("../samples/wellness_plan.txt")),
];

// The pre-built tag store (bincode). Its associations reference the sha256 of
// the embedded prompt files above.
const TAGS_DB: &[u8] = include_bytes!("../samples/tags_db");

/// Ensure the user's `~/.upl` library exists. If `~/.upl` does not exist yet,
/// create it together with `~/.upl/prompts/`, drop the bundled sample prompts
/// there, and write the bundled `tags_db` to `~/.upl/tags_db`.
///
/// If `~/.upl` already exists this is a no-op: nothing is overwritten, so a
/// user who has already customized their library is never disturbed.
pub fn ensure() -> io::Result<()> {
    let home = upl_home()?;

    // Only seed on the very first run, when ~/.upl is absent.
    if home.exists() {
        return Ok(());
    }

    let prompts_dir = home.join("prompts");
    fs::create_dir_all(&prompts_dir)?;

    for (name, contents) in SAMPLES {
        fs::write(prompts_dir.join(name), contents)?;
    }

    fs::write(home.join("tags_db"), TAGS_DB)?;

    Ok(())
}

#[cfg(test)]
#[path = "../test/unit/seed.rs"]
mod tests;
