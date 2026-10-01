use mlua::{Lua, Table};

use crate::plugin::{Match, classify_ci};

/// Ranks `candidates` against `query` with the built-ins' own match kinds; each
/// hit is `{index, kind}`, best first, `index` 1-based into the input table.
pub(super) fn lib(lua: &Lua) -> mlua::Result<Table> {
    let fuzzy = lua.create_table()?;
    fuzzy.set(
        "match",
        lua.create_function(|lua, (query, candidates): (String, Table)| {
            let mut hits: Vec<(u32, usize, Match)> = Vec::new();
            for (at, candidate) in candidates.sequence_values::<String>().enumerate() {
                if let Some(kind) = classify_ci(&candidate?, &query) {
                    hits.push((kind.weight(), at, kind));
                }
            }
            hits.sort_by_key(|hit| std::cmp::Reverse(hit.0));

            let reply = lua.create_table()?;
            for (at, (_, index, kind)) in hits.into_iter().enumerate() {
                let hit = lua.create_table()?;
                hit.set("index", index as i64 + 1)?;
                hit.set("kind", kind_name(kind))?;
                reply.set(at + 1, hit)?;
            }
            Ok(reply)
        })?,
    )?;
    Ok(fuzzy)
}

fn kind_name(kind: Match) -> &'static str {
    match kind {
        Match::Exact => "exact",
        Match::Prefix => "prefix",
        Match::Word => "word",
        Match::Substring => "substring",
        Match::Loose => "loose",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits_rank_by_kind_and_keep_input_order() {
        let lua = Lua::new();
        lua.globals().set("fuzzy", lib(&lua).unwrap()).unwrap();
        let hits: String = lua
            .load(
                r#"
                local candidates = {
                  "digit", "git", "github", "gitlab", "my git repo", "gifted", "nothing",
                }
                local out = {}
                for _, hit in ipairs(fuzzy.match("git", candidates)) do
                  out[#out + 1] = hit.kind .. ":" .. hit.index
                end
                return table.concat(out, ",")
                "#,
            )
            .eval()
            .unwrap();
        assert_eq!(hits, "exact:2,prefix:3,prefix:4,word:5,substring:1,loose:6");
    }

    #[test]
    fn an_empty_query_or_list_matches_nothing() {
        let lua = Lua::new();
        lua.globals().set("fuzzy", lib(&lua).unwrap()).unwrap();
        let empty_query: i64 = lua
            .load(r#"return #fuzzy.match("", { "a" })"#)
            .eval()
            .unwrap();
        assert_eq!(empty_query, 0);
        let empty_list: i64 = lua
            .load(r#"return #fuzzy.match("git", {})"#)
            .eval()
            .unwrap();
        assert_eq!(empty_list, 0);
    }
}
