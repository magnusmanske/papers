//! Tests for [`crate::generic_author_info::GenericAuthorInfo`].
//!
//! Lives in a sibling file rather than at the bottom of `mod.rs` so the
//! production code isn't drowned in test cases (the parent module was
//! 1300 LOC, ~700 of which were tests). The module is `#[cfg(test)]`
//! so it's only compiled for `cargo test`.

use super::*;

#[test]
fn asciify_string() {
    let ga = GenericAuthorInfo::new();
    assert_eq!(ga.asciify_string("äöüáàâéèñïçß"), "aouaaaeenicss");
}

#[test]
fn author_names_match() {
    let ga = GenericAuthorInfo::new();
    assert_eq!(ga.author_names_match("Manske M", "Manske HM"), 1);
    assert_eq!(ga.author_names_match("Manske M", "HM Manske"), 1);
    assert_eq!(ga.author_names_match("Heinrich M Manske", "manske heinrich"), 2);
    assert_eq!(ga.author_names_match("Notmyname M Manske", "Heinrich M Manske"), 1);
}

#[test]
fn compare_by_item() {
    let mut ga1 = GenericAuthorInfo::new();
    let mut ga2 = GenericAuthorInfo::new();
    assert_eq!(ga1.compare(&ga2), 0);
    ga1.wikidata_item = Some("Q1".to_string());
    assert_eq!(ga1.compare(&ga2), 0);
    ga2.wikidata_item = Some("Q1".to_string());
    assert_eq!(ga1.compare(&ga2), SCORE_ITEM_MATCH);
    ga1.wikidata_item = Some("Q2".to_string());
    assert_eq!(ga1.compare(&ga2), 0);
}

#[test]
fn compare_by_props() {
    let mut ga1 = GenericAuthorInfo::new();
    let mut ga2 = GenericAuthorInfo::new();
    assert_eq!(ga1.compare(&ga2), 0);
    ga1.prop2id.insert("foo".to_string(), "bar".to_string());
    assert_eq!(ga1.compare(&ga2), 0);
    ga2.prop2id.insert("foo".to_string(), "bar".to_string());
    assert_eq!(ga1.compare(&ga2), SCORE_PROP_MATCH);
    ga1.prop2id.insert("foo2".to_string(), "bar2".to_string());
    ga2.prop2id.insert("foo2".to_string(), "bar2".to_string());
    assert_eq!(ga1.compare(&ga2), SCORE_PROP_MATCH * 2);
}

#[test]
fn compare_by_name() {
    let mut ga1 = GenericAuthorInfo::new();
    let mut ga2 = GenericAuthorInfo::new();
    assert_eq!(ga1.compare(&ga2), 0);
    ga1.name = Some("Heinrich M Manske".to_string());
    assert_eq!(ga1.compare(&ga2), 0);
    ga2.name = Some("Manske Heinrich M".to_string());
    assert_eq!(ga1.compare(&ga2), SCORE_NAME_MATCH * 2);
    ga1.name = Some("Manske HM".to_string());
    assert_eq!(ga1.compare(&ga2), SCORE_NAME_MATCH);
}

#[test]
fn compare_by_list_number() {
    let mut ga1 = GenericAuthorInfo::new();
    let mut ga2 = GenericAuthorInfo::new();
    assert_eq!(ga1.compare(&ga2), 0);
    ga1.list_number = Some("123".to_string());
    assert_eq!(ga1.compare(&ga2), 0);
    ga2.list_number = Some("123".to_string());
    assert_eq!(ga1.compare(&ga2), SCORE_LIST_NUMBER);
    ga1.name = Some("Foobar Baz".to_string());
    ga2.name = Some("Foobar B.".to_string());
    assert_eq!(ga1.compare(&ga2), SCORE_LIST_NUMBER_AND_NAME + SCORE_NAME_MATCH);
    ga1.name = None;
    ga2.name = None;
    ga1.list_number = Some("456".to_string());
    assert_eq!(ga1.compare(&ga2), 0);
}

#[test]
fn create_author_statement_in_paper_item() {
    let mut item = Entity::new_empty_item();
    let ga = GenericAuthorInfo::new();
    ga.create_author_statement_in_paper_item(&mut item);
    assert!(item.claims().is_empty());

    let mut item = Entity::new_empty_item();
    let mut ga = GenericAuthorInfo::new();
    ga.name = Some("Magnus Manske".to_string());
    ga.create_author_statement_in_paper_item(&mut item);
    assert_eq!(item.claims().len(), 1);
    assert_eq!(item.claims()[0].main_snak().property(), "P2093");
    assert!(item.claims()[0].qualifiers().is_empty());

    ga.list_number = Some("123".to_string());
    let mut item = Entity::new_empty_item();
    ga.create_author_statement_in_paper_item(&mut item);
    assert_eq!(item.claims().len(), 1);
    assert_eq!(item.claims()[0].qualifiers().len(), 1);

    ga.wikidata_item = Some("Q12345".to_string());
    let mut item = Entity::new_empty_item();
    ga.create_author_statement_in_paper_item(&mut item);
    assert_eq!(item.claims().len(), 1);
    assert_eq!(item.claims()[0].main_snak().property(), "P50");
    let qualifiers = item.claims()[0].qualifiers();
    assert_eq!(qualifiers[0], Snak::new_string("P1545", "123"));
    assert_eq!(qualifiers[1], Snak::new_string("P1932", "Magnus Manske"));
}

#[test]
fn amend_author_item() {
    let mut ga = GenericAuthorInfo::new();
    ga.name = Some("Magnus Manske".to_string());
    ga.alternative_names.push("HM Manske".to_string());
    ga.prop2id.insert("P496".to_string(), "1234-5678-1234-5678".to_string());
    let mut item = Entity::new_empty_item();
    ga.amend_author_item(&mut item);
    assert_eq!(item.label_in_locale("en"), Some("Magnus Manske"));
    // Note: alternative_names are no longer pushed as Wikidata aliases
    // (the dead add_aliases() const fn was removed). The field stays
    // populated as test-observable state via merge_from.
    assert!(item.aliases().is_empty(), "amend_author_item should not push aliases anymore");
    assert_eq!(*item.claims()[0].main_snak(), Snak::new_item("P31", "Q5"));
    assert_eq!(*item.claims()[1].main_snak(), Snak::new_item("P106", "Q1650915"));
    assert_eq!(
        *item.claims()[2].main_snak(),
        Snak::new_external_id("P496", "1234-5678-1234-5678")
    );
}

#[test]
fn find_best_match() {
    let mut ga_main = GenericAuthorInfo::new();
    ga_main.name = Some("Magnus Manske".to_string());

    let mut vector: Vec<GenericAuthorInfo> = vec![];
    let mut ga = GenericAuthorInfo::new();
    ga.name = Some("Manske M".to_string());
    vector.push(ga);
    let mut ga = GenericAuthorInfo::new();
    ga.name = Some("Bar Fo".to_string());
    vector.push(ga);

    assert_eq!(ga_main.find_best_match(&vector), None);

    // Again

    let mut ga_main = GenericAuthorInfo::new();
    ga_main.name = Some("Magnus Manske".to_string());
    ga_main.list_number = Some(123.to_string());

    let mut vector: Vec<GenericAuthorInfo> = vec![];
    let mut ga = GenericAuthorInfo::new();
    ga.name = Some("Manske M".to_string());
    ga.list_number = Some(123.to_string());
    vector.push(ga);
    let mut ga = GenericAuthorInfo::new();
    ga.name = Some("Bar Fo".to_string());
    ga.list_number = Some(456.to_string());
    vector.push(ga);

    assert_eq!(
        ga_main.find_best_match(&vector),
        Some((0, SCORE_NAME_MATCH + SCORE_LIST_NUMBER))
    );

    // Again

    let mut ga_main = GenericAuthorInfo::new();
    ga_main.name = Some("Magnus Manske".to_string());
    ga_main.list_number = Some(123.to_string());

    let mut vector: Vec<GenericAuthorInfo> = vec![];
    let mut ga = GenericAuthorInfo::new();
    ga.name = Some("Manske M".to_string());
    ga.list_number = Some(456.to_string());
    vector.push(ga);
    let mut ga = GenericAuthorInfo::new();
    ga.name = Some("Bar Fo".to_string());
    ga.list_number = Some(123.to_string());
    vector.push(ga);

    assert_eq!(ga_main.find_best_match(&vector), None);
}

// --- merge_from tests ---

#[test]
fn merge_from_basic_name_and_props() {
    let mut ga1 = GenericAuthorInfo::new();
    ga1.name = Some("John Smith".to_string());
    ga1.list_number = Some("1".to_string());

    let mut ga2 = GenericAuthorInfo::new();
    ga2.name = Some("J Smith".to_string());
    ga2.list_number = Some("1".to_string());
    ga2.prop2id.insert("P496".to_string(), "0000-1234-5678-9012".to_string());

    assert!(ga1.merge_from(&ga2).is_ok());
    assert_eq!(ga1.name, Some("John Smith".to_string())); // primary name kept
    assert!(ga1.alternative_names.contains(&"J Smith".to_string())); // secondary added as alias
    assert_eq!(ga1.prop2id.get("P496"), Some(&"0000-1234-5678-9012".to_string())); // prop absorbed
    assert_eq!(ga1.list_number, Some("1".to_string()));
}

#[test]
fn merge_from_conflicting_list_numbers_succeeds() {
    // Previously this would return Err; now it should succeed and keep existing
    // number.
    let mut ga1 = GenericAuthorInfo::new();
    ga1.name = Some("John Smith".to_string());
    ga1.list_number = Some("1".to_string());

    let mut ga2 = GenericAuthorInfo::new();
    ga2.name = Some("John Smith".to_string());
    ga2.list_number = Some("3".to_string()); // Different position in another source

    assert!(ga1.merge_from(&ga2).is_ok());
    assert_eq!(ga1.list_number, Some("1".to_string())); // Existing kept
}

#[test]
fn merge_from_conflicting_prop2id_succeeds_keeps_existing() {
    // Previously this would return Err; now it should succeed and keep existing
    // value.
    let mut ga1 = GenericAuthorInfo::new();
    ga1.prop2id.insert("P496".to_string(), "0000-1111-2222-3333".to_string());

    let mut ga2 = GenericAuthorInfo::new();
    ga2.prop2id.insert("P496".to_string(), "9999-8888-7777-6666".to_string()); // Conflict
    ga2.prop2id.insert("P1053".to_string(), "A-1234-5678".to_string()); // New, no conflict

    assert!(ga1.merge_from(&ga2).is_ok());
    // Conflicting property: keep existing (higher-priority source)
    assert_eq!(ga1.prop2id.get("P496"), Some(&"0000-1111-2222-3333".to_string()));
    // Non-conflicting property: absorbed
    assert_eq!(ga1.prop2id.get("P1053"), Some(&"A-1234-5678".to_string()));
}

#[test]
fn merge_from_different_wikidata_items_still_fails() {
    // Different Q-items means different people; this should remain an Err.
    let mut ga1 = GenericAuthorInfo::new();
    ga1.wikidata_item = Some("Q1".to_string());

    let mut ga2 = GenericAuthorInfo::new();
    ga2.wikidata_item = Some("Q2".to_string());

    assert!(ga1.merge_from(&ga2).is_err());
    assert_eq!(ga1.wikidata_item, Some("Q1".to_string())); // Unchanged after failure
}

#[test]
fn merge_from_adopts_wikidata_item_if_missing() {
    let mut ga1 = GenericAuthorInfo::new();
    ga1.name = Some("John Smith".to_string());
    // ga1 has no wikidata_item

    let mut ga2 = GenericAuthorInfo::new();
    ga2.wikidata_item = Some("Q42".to_string());
    ga2.prop2id.insert("P496".to_string(), "0000-0001-2345-6789".to_string());

    assert!(ga1.merge_from(&ga2).is_ok());
    assert_eq!(ga1.wikidata_item, Some("Q42".to_string())); // Adopted from ga2
    assert_eq!(ga1.prop2id.get("P496"), Some(&"0000-0001-2345-6789".to_string()));
}

#[test]
fn merge_from_adopts_list_number_if_missing() {
    let mut ga1 = GenericAuthorInfo::new();
    ga1.name = Some("Jane Doe".to_string());
    // No list_number (e.g., from ORCID)

    let mut ga2 = GenericAuthorInfo::new();
    ga2.name = Some("Jane Doe".to_string());
    ga2.list_number = Some("2".to_string());

    assert!(ga1.merge_from(&ga2).is_ok());
    assert_eq!(ga1.list_number, Some("2".to_string())); // Adopted
}

// --- compare exact name bonus tests ---

#[test]
fn compare_exact_name_scores_higher_than_partial() {
    // "John Smith" exactly matches "John Smith" → gets +1 bonus
    let mut ga_ref = GenericAuthorInfo::new();
    ga_ref.name = Some("John Smith".to_string());

    let mut ga_exact = GenericAuthorInfo::new();
    ga_exact.name = Some("John Smith".to_string());

    let mut ga_partial = GenericAuthorInfo::new();
    ga_partial.name = Some("J Smith".to_string()); // only "smith" matches, no exact bonus

    let score_exact = ga_ref.compare(&ga_exact);
    let score_partial = ga_ref.compare(&ga_partial);
    assert!(score_exact > score_partial, "Exact name match must outscore partial");
    // 101 (2 words × 50 + 1 bonus) vs 50 (1 word match, no bonus)
    assert_eq!(score_exact, SCORE_NAME_MATCH * 2 + 1);
    assert_eq!(score_partial, SCORE_NAME_MATCH);
}

#[test]
fn compare_exact_name_bonus_isolated() {
    // Names under 3 chars don't trigger word-matching but do trigger the exact
    // bonus, cleanly isolating the +1 effect.
    let mut ga_ref = GenericAuthorInfo::new();
    ga_ref.name = Some("Li".to_string()); // 2-char word: filtered by \w{3,} regex

    let mut ga_same = GenericAuthorInfo::new();
    ga_same.name = Some("Li".to_string());

    let mut ga_different = GenericAuthorInfo::new();
    ga_different.name = Some("Lo".to_string());

    assert_eq!(ga_ref.compare(&ga_same), 1, "Only the +1 exact bonus contributes");
    assert_eq!(ga_ref.compare(&ga_different), 0, "No match, no bonus");
}

#[test]
fn compare_exact_name_bonus_applies_after_asciification() {
    // Accent-normalized names should get the exact name bonus
    let mut ga1 = GenericAuthorInfo::new();
    ga1.name = Some("Müller Hans".to_string());

    let mut ga_accent = GenericAuthorInfo::new();
    ga_accent.name = Some("Müller Hans".to_string()); // Identical

    let mut ga_ascii = GenericAuthorInfo::new();
    ga_ascii.name = Some("Muller Hans".to_string()); // ASCII equivalent

    // Both should get the exact match bonus (ü → u after asciification)
    assert_eq!(ga1.compare(&ga_accent), ga1.compare(&ga_ascii));
}

// --- find_best_match with exact name tiebreaker ---

#[test]
fn find_best_match_exact_name_breaks_surname_tie() {
    // Without list numbers (e.g., ORCID data), "Li Wang" and "Min Wang" both score
    // 50 via the shared surname "Wang". The exact name bonus should break the tie
    // and correctly identify "Li Wang" as the matching entry.
    let mut ga_new = GenericAuthorInfo::new();
    ga_new.name = Some("Li Wang".to_string());

    let ga_li = {
        let mut g = GenericAuthorInfo::new();
        g.name = Some("Li Wang".to_string());
        g
    };
    let ga_min = {
        let mut g = GenericAuthorInfo::new();
        g.name = Some("Min Wang".to_string());
        g
    };

    let result = ga_new.find_best_match(&[ga_li, ga_min]);
    // Should match ga_li (index 0) with score 51 (50 + 1 bonus)
    assert_eq!(result, Some((0, SCORE_NAME_MATCH + 1)));
}

#[test]
fn find_best_match_still_returns_none_for_ambiguous_different_names() {
    // "Min Wang" vs ["Li Wang", "Min Wang"]: the exact name bonus distinguishes
    // them. "min" (3 chars) + "wang" (4 chars) → 2-word match = 100, + 1
    // bonus = 101 for "Min Wang". "wang" only (1-word match, no bonus) = 50
    // for "Li Wang".
    let mut ga_new = GenericAuthorInfo::new();
    ga_new.name = Some("Min Wang".to_string());

    let ga_li = {
        let mut g = GenericAuthorInfo::new();
        g.name = Some("Li Wang".to_string());
        g
    };
    let ga_min = {
        let mut g = GenericAuthorInfo::new();
        g.name = Some("Min Wang".to_string());
        g
    };

    let result = ga_new.find_best_match(&[ga_li, ga_min]);
    // "min" is 3 chars so it counts; 2 word matches + bonus = 101
    assert_eq!(result, Some((1, SCORE_NAME_MATCH * 2 + 1)));
}

// --- has_partial_match tests ---

#[test]
fn has_partial_match_empty_list() {
    let ga = GenericAuthorInfo::new();
    assert!(!ga.has_partial_match(&[]));
}

#[test]
fn has_partial_match_list_number_only_is_not_partial() {
    // Score of exactly SCORE_LIST_NUMBER (5) should NOT count as a partial match.
    // The check is strictly >, so 5 > 5 is false.
    let mut ga = GenericAuthorInfo::new();
    ga.list_number = Some("1".to_string());

    let mut other = GenericAuthorInfo::new();
    other.list_number = Some("1".to_string()); // Same position, no name overlap

    assert!(!ga.has_partial_match(&[other]));
}

#[test]
fn has_partial_match_true_for_name_overlap() {
    // A shared surname (score 50) is a meaningful partial match.
    let mut ga = GenericAuthorInfo::new();
    ga.name = Some("J Smith".to_string());

    let mut other = GenericAuthorInfo::new();
    other.name = Some("John Smith".to_string());

    assert!(ga.has_partial_match(&[other]));
}

#[test]
fn has_partial_match_true_for_prop_match() {
    // A matching external ID (score 90) is a strong partial match.
    let mut ga = GenericAuthorInfo::new();
    ga.prop2id.insert("P496".to_string(), "0000-0001-2345-6789".to_string());

    let mut other = GenericAuthorInfo::new();
    other.prop2id.insert("P496".to_string(), "0000-0001-2345-6789".to_string());

    assert!(ga.has_partial_match(&[other]));
}

#[test]
fn has_partial_match_false_for_completely_different_authors() {
    let mut ga = GenericAuthorInfo::new();
    ga.name = Some("Li Wang".to_string());
    ga.list_number = Some("3".to_string());

    let other = GenericAuthorInfo::new_from_name_num("John Smith", 1);

    assert!(!ga.has_partial_match(&[other]));
}

#[test]
fn has_partial_match_checks_all_candidates() {
    // Should return true even if only the second candidate is a partial match.
    let mut ga = GenericAuthorInfo::new();
    ga.name = Some("Jane Doe".to_string());

    let unrelated = GenericAuthorInfo::new_from_name_num("Bob Jones", 1);
    let mut matching = GenericAuthorInfo::new();
    matching.name = Some("J Doe".to_string()); // shares "doe"

    assert!(!ga.has_partial_match(std::slice::from_ref(&unrelated)));
    assert!(ga.has_partial_match(&[unrelated, matching]));
}

// --- deduplicate tests ---

#[test]
fn deduplicate_empty_list() {
    let mut authors: Vec<GenericAuthorInfo> = vec![];
    GenericAuthorInfo::deduplicate(&mut authors);
    assert!(authors.is_empty());
}

#[test]
fn deduplicate_single_entry_unchanged() {
    let mut authors = vec![GenericAuthorInfo::new_from_name_num("John Smith", 1)];
    GenericAuthorInfo::deduplicate(&mut authors);
    assert_eq!(authors.len(), 1);
    assert_eq!(authors[0].name, Some("John Smith".to_string()));
}

#[test]
fn deduplicate_distinct_authors_unchanged() {
    let mut authors = vec![
        GenericAuthorInfo::new_from_name_num("John Smith", 1),
        GenericAuthorInfo::new_from_name_num("Jane Doe", 2),
        GenericAuthorInfo::new_from_name_num("Bob Jones", 3),
    ];
    GenericAuthorInfo::deduplicate(&mut authors);
    assert_eq!(authors.len(), 3);
}

#[test]
fn deduplicate_removes_exact_duplicate() {
    let mut authors = vec![
        GenericAuthorInfo::new_from_name_num("John Smith", 1),
        GenericAuthorInfo::new_from_name_num("John Smith", 1), // Exact duplicate
    ];
    GenericAuthorInfo::deduplicate(&mut authors);
    assert_eq!(authors.len(), 1);
    assert_eq!(authors[0].name, Some("John Smith".to_string()));
}

#[test]
fn deduplicate_abbreviated_name_same_list_number() {
    // "John Smith"(1) and "J Smith"(1): score = 50 + 30 = 80 → merged.
    // Earlier entry's name is kept; later becomes an alternative.
    let mut authors = vec![
        GenericAuthorInfo::new_from_name_num("John Smith", 1),
        GenericAuthorInfo::new_from_name_num("J Smith", 1),
    ];
    GenericAuthorInfo::deduplicate(&mut authors);
    assert_eq!(authors.len(), 1);
    assert_eq!(authors[0].name, Some("John Smith".to_string()));
    assert!(authors[0].alternative_names.contains(&"J Smith".to_string()));
}

#[test]
fn deduplicate_absorbs_external_ids_from_duplicate() {
    // The later entry (lower priority) has an ORCID; after dedup it should be on
    // the first.
    let first = GenericAuthorInfo::new_from_name_num("John Smith", 1);
    let mut second = GenericAuthorInfo::new_from_name_num("John Smith", 1);
    second.prop2id.insert("P496".to_string(), "0000-0001-2345-6789".to_string());

    let mut authors = vec![first, second];
    GenericAuthorInfo::deduplicate(&mut authors);
    assert_eq!(authors.len(), 1);
    assert_eq!(authors[0].prop2id.get("P496"), Some(&"0000-0001-2345-6789".to_string()));
}

#[test]
fn deduplicate_preserves_first_entry_data() {
    // First entry's data must win; second entry's conflicting data is discarded.
    let mut first = GenericAuthorInfo::new_from_name_num("John Smith", 1);
    first.prop2id.insert("P496".to_string(), "FIRST-ORCID".to_string());

    let mut second = GenericAuthorInfo::new_from_name_num("John Smith", 1);
    second.prop2id.insert("P496".to_string(), "SECOND-ORCID".to_string()); // Conflict: first wins

    let mut authors = vec![first, second];
    GenericAuthorInfo::deduplicate(&mut authors);
    assert_eq!(authors.len(), 1);
    assert_eq!(authors[0].prop2id.get("P496"), Some(&"FIRST-ORCID".to_string()));
}

#[test]
fn deduplicate_tolerates_different_list_numbers_when_names_match_strongly() {
    // Two authors with the same full name but different list numbers (from sources
    // that disagree on ordering) should be merged; the first entry's list
    // number is kept.
    let first = GenericAuthorInfo::new_from_name_num("Heinrich Manske", 1);
    let mut second = GenericAuthorInfo::new_from_name_num("Heinrich Manske", 3); // shifted position
    second.prop2id.insert("P496".to_string(), "0000-0001-2345-6789".to_string());

    let mut authors = vec![first.clone(), second];
    GenericAuthorInfo::deduplicate(&mut authors);
    assert_eq!(authors.len(), 1);
    assert_eq!(authors[0].list_number, first.list_number); // First's list number kept
    assert!(authors[0].prop2id.contains_key("P496")); // ORCID absorbed
}

// --- initials conflict detection tests ---

#[test]
fn author_names_match_rejects_conflicting_first_initials() {
    let ga = GenericAuthorInfo::new();
    // Cases from the false-positive Wikidata edit:
    assert_eq!(ga.author_names_match("Bruce Allen", "G. Allen"), 0);
    assert_eq!(ga.author_names_match("Jonathan Anderson", "S. B. Anderson"), 0);
    assert_eq!(ga.author_names_match("Carl Blair", "D. G. Blair"), 0);
    assert_eq!(ga.author_names_match("R Gustafson", "E. K. Gustafson"), 0);
    assert_eq!(ga.author_names_match("Andrew M. Hopkins", "P. Hopkins"), 0);
    assert_eq!(ga.author_names_match("Bryn Jones", "D. I. Jones"), 0);
}

#[test]
fn author_names_match_allows_compatible_initials() {
    let ga = GenericAuthorInfo::new();
    // Initial matches first letter of full name
    assert_eq!(ga.author_names_match("John Smith", "J. Smith"), 1);
    assert_eq!(ga.author_names_match("J Smith", "John Smith"), 1);
    // Multiple initials, one matches
    assert_eq!(ga.author_names_match("John Smith", "J. A. Smith"), 1);
}

#[test]
fn author_names_match_no_conflict_when_one_side_fully_matched() {
    let ga = GenericAuthorInfo::new();
    // "Heinrich Manske" vs "manske heinrich" — all long words match, no remaining
    // parts
    assert_eq!(ga.author_names_match("Heinrich Manske", "manske heinrich"), 2);
}

#[test]
fn conflicting_initials_prevents_false_match_with_list_number() {
    // "Bruce Allen"(5) vs "G. Allen"(5): should NOT match even with same list
    // number.
    let mut ga1 = GenericAuthorInfo::new();
    ga1.name = Some("Bruce Allen".to_string());
    ga1.list_number = Some("5".to_string());

    let mut ga2 = GenericAuthorInfo::new();
    ga2.name = Some("G. Allen".to_string());
    ga2.list_number = Some("5".to_string());

    let score = ga1.compare(&ga2);
    assert!(
        score < SCORE_MATCH_MIN,
        "Conflicting initials must not produce a match (score {} >= {})",
        score,
        SCORE_MATCH_MIN
    );
}

// --- names_have_conflicting_initials tests ---

#[test]
fn names_have_conflicting_initials_ck_clarke_vs_jenny_clarke() {
    let ga = GenericAuthorInfo::new();
    let n1 = ga.asciify_string("Ck Clarke").replace('.', " ");
    let n2 = ga.asciify_string("Jenny Clarke").replace('.', " ");
    assert!(GenericAuthorInfo::names_have_conflicting_initials(&n1, &n2));
}

#[test]
fn names_have_conflicting_initials_compatible() {
    let ga = GenericAuthorInfo::new();
    let n1 = ga.asciify_string("J. Smith").replace('.', " ");
    let n2 = ga.asciify_string("John Smith").replace('.', " ");
    assert!(!GenericAuthorInfo::names_have_conflicting_initials(&n1, &n2));
}

#[test]
fn names_have_conflicting_initials_fully_matched() {
    let ga = GenericAuthorInfo::new();
    let n1 = ga.asciify_string("Heinrich Manske").replace('.', " ");
    let n2 = ga.asciify_string("manske heinrich").replace('.', " ");
    assert!(!GenericAuthorInfo::names_have_conflicting_initials(&n1, &n2));
}

#[test]
fn names_have_conflicting_initials_both_initials_conflict() {
    let ga = GenericAuthorInfo::new();
    let n1 = ga.asciify_string("A B Smith").replace('.', " ");
    let n2 = ga.asciify_string("C D Smith").replace('.', " ");
    assert!(GenericAuthorInfo::names_have_conflicting_initials(&n1, &n2));
}

#[test]
fn names_have_conflicting_initials_partial_initial_overlap() {
    let ga = GenericAuthorInfo::new();
    // "C K Smith" vs "C J Smith" — 'c' overlaps, so no conflict
    let n1 = ga.asciify_string("C K Smith").replace('.', " ");
    let n2 = ga.asciify_string("C J Smith").replace('.', " ");
    assert!(!GenericAuthorInfo::names_have_conflicting_initials(&n1, &n2));
}

// --- Bug scenario tests ---

#[test]
fn ck_clarke_vs_jenny_clarke_no_false_match() {
    // The exact scenario from the bug report
    let mut ga1 = GenericAuthorInfo::new();
    ga1.name = Some("Ck Clarke".to_string());
    ga1.list_number = Some("3".to_string());

    let mut ga2 = GenericAuthorInfo::new();
    ga2.name = Some("Jenny Clarke".to_string());
    ga2.list_number = Some("3".to_string());

    let score = ga1.compare(&ga2);
    // With conflicting initials, should get only SCORE_LIST_NUMBER (5)
    assert_eq!(score, SCORE_LIST_NUMBER);
    // has_partial_match: 5 > 5 is false — no false partial match
    assert!(!ga1.has_partial_match(&[ga2]));
}

#[test]
fn ck_clarke_author_names_match_rejects() {
    let ga = GenericAuthorInfo::new();
    assert_eq!(ga.author_names_match("Ck Clarke", "Jenny Clarke"), 0);
}

#[test]
fn compare_list_number_and_name_with_compatible_initials() {
    // J Smith at position 1 should still match John Smith at position 1
    let mut ga1 = GenericAuthorInfo::new();
    ga1.name = Some("J Smith".to_string());
    ga1.list_number = Some("1".to_string());

    let mut ga2 = GenericAuthorInfo::new();
    ga2.name = Some("John Smith".to_string());
    ga2.list_number = Some("1".to_string());

    let score = ga1.compare(&ga2);
    assert_eq!(score, SCORE_LIST_NUMBER_AND_NAME + SCORE_NAME_MATCH);
}

#[test]
fn compare_list_number_conflicting_initials_only_gets_list_number() {
    // R Gustafson at position 2 vs E. K. Gustafson at position 2
    let mut ga1 = GenericAuthorInfo::new();
    ga1.name = Some("R Gustafson".to_string());
    ga1.list_number = Some("2".to_string());

    let mut ga2 = GenericAuthorInfo::new();
    ga2.name = Some("E. K. Gustafson".to_string());
    ga2.list_number = Some("2".to_string());

    let score = ga1.compare(&ga2);
    // Conflicting initials: only SCORE_LIST_NUMBER
    assert_eq!(score, SCORE_LIST_NUMBER);
    assert!(!ga1.has_partial_match(&[ga2]));
}

// TODO:
// fn new_from_statement
// fn get_or_create_author_item(
// fn update_author_item(
