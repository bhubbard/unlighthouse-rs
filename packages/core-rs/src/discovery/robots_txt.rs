/// Port of packages/core/src/util/robotsTxtParser.ts

#[derive(Debug, Clone)]
pub struct RobotsRule {
    pub pattern: String,
    pub allow: bool,
}

#[derive(Debug, Clone)]
pub struct RobotsGroup {
    pub user_agent: Vec<String>,
    pub allow: Vec<String>,
    pub disallow: Vec<String>,
    pub rules: Vec<RobotsRule>,
    pub indexable: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ParsedRobotsTxt {
    pub groups: Vec<RobotsGroup>,
    pub sitemaps: Vec<String>,
}

/// Parse a robots.txt file into groups + sitemap URLs.
/// Follows the Google specification.
pub fn parse_robots_txt(content: &str) -> ParsedRobotsTxt {
    let mut groups: Vec<RobotsGroup> = Vec::new();
    let mut sitemaps: Vec<String> = Vec::new();
    let mut create_new_group = false;

    let mut current_user_agents: Vec<String> = Vec::new();
    let mut current_allow: Vec<String> = Vec::new();
    let mut current_disallow: Vec<String> = Vec::new();
    let mut current_host: Option<String> = None;

    let flush_group = |ua: &mut Vec<String>,
                       allow: &mut Vec<String>,
                       disallow: &mut Vec<String>,
                       _host: &mut Option<String>,
                       groups: &mut Vec<RobotsGroup>| {
        let disallow_filtered: Vec<String> = disallow.iter().cloned().collect();
        let allow_filtered: Vec<String> = allow.iter().filter(|r| !r.is_empty()).cloned().collect();

        let mut rules: Vec<RobotsRule> = Vec::new();
        for r in disallow_filtered.iter().filter(|r| !r.is_empty()) {
            rules.push(RobotsRule { pattern: r.clone(), allow: false });
        }
        for r in &allow_filtered {
            rules.push(RobotsRule { pattern: r.clone(), allow: true });
        }

        let indexable = !disallow_filtered.contains(&"/".to_string());
        let user_agent = if ua.is_empty() { vec!["*".to_string()] } else { ua.clone() };

        groups.push(RobotsGroup {
            user_agent,
            allow: allow_filtered,
            disallow: disallow_filtered,
            rules,
            indexable,
        });

        ua.clear();
        allow.clear();
        disallow.clear();
    };

    for line in content.lines() {
        let sep = match line.find(':') {
            Some(i) => i,
            None => continue,
        };
        let rule = line[..sep].trim();
        let val = line[sep + 1..].trim().to_string();

        match rule {
            "User-agent" => {
                if create_new_group {
                    flush_group(
                        &mut current_user_agents,
                        &mut current_allow,
                        &mut current_disallow,
                        &mut current_host,
                        &mut groups,
                    );
                    create_new_group = false;
                }
                current_user_agents.push(val);
            }
            "Allow" => {
                current_allow.push(val);
                create_new_group = true;
            }
            "Disallow" => {
                current_disallow.push(val);
                create_new_group = true;
            }
            "Sitemap" => {
                sitemaps.push(val);
            }
            "Host" => {
                current_host = Some(val);
            }
            _ => {}
        }
    }

    // flush final group
    flush_group(
        &mut current_user_agents,
        &mut current_allow,
        &mut current_disallow,
        &mut current_host,
        &mut groups,
    );

    ParsedRobotsTxt { groups, sitemaps }
}

/// Port of the Google wildcard matching algorithm from robots.ts.
/// `*` matches any sequence of characters; `$` anchors to the end of the path.
pub fn matches_pattern(pattern: &str, path: &str) -> bool {
    let anchored = pattern.ends_with('$');
    let pat = if anchored { &pattern[..pattern.len() - 1] } else { pattern };

    let parts: Vec<&str> = pat.split('*').collect();

    let mut pos = 0usize;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        let path_bytes = path.as_bytes();
        let part_bytes = part.as_bytes();
        let first = i == 0;

        if first {
            // The first segment must match at the start
            if !path_bytes.starts_with(part_bytes) {
                return false;
            }
            pos = part.len();
        } else {
            // Find the part in path starting from pos
            let slice = &path[pos..];
            match slice.find(part) {
                Some(idx) => {
                    pos += idx + part.len();
                }
                None => return false,
            }
        }
    }

    if anchored {
        // remaining path must be empty
        pos == path.len()
    } else {
        true
    }
}

/// Given a list of rules, find the longest-matching rule for `path`.
/// Longer patterns win (Google's spec).
pub fn match_path_to_rule<'a>(path: &str, rules: &'a [RobotsRule]) -> Option<&'a RobotsRule> {
    let mut best: Option<&RobotsRule> = None;
    let mut best_len = 0usize;

    for rule in rules {
        if matches_pattern(&rule.pattern, path) {
            let len = rule.pattern.trim_end_matches('$').len();
            if len >= best_len {
                best_len = len;
                best = Some(rule);
            }
        }
    }

    best
}

/// Collect all disallow rules from groups that apply to Googlebot (or `*`).
pub fn collect_disallow_rules(parsed: &ParsedRobotsTxt) -> Vec<RobotsRule> {
    let mut rules = Vec::new();
    for group in &parsed.groups {
        let applies = group.user_agent.iter().any(|ua| ua == "*" || ua.to_lowercase().contains("googlebot"));
        if applies {
            for rule in &group.rules {
                if !rule.allow {
                    rules.push(RobotsRule {
                        pattern: rule.pattern.clone(),
                        allow: false,
                    });
                }
            }
        }
    }
    rules
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic() {
        let txt = "User-agent: *\nDisallow: /admin\nSitemap: https://example.com/sitemap.xml\n";
        let parsed = parse_robots_txt(txt);
        assert_eq!(parsed.sitemaps, vec!["https://example.com/sitemap.xml"]);
        assert_eq!(parsed.groups.len(), 1);
        assert!(parsed.groups[0].disallow.contains(&"/admin".to_string()));
    }

    #[test]
    fn test_matches_wildcard() {
        assert!(matches_pattern("/admin*", "/admin/dashboard"));
        assert!(matches_pattern("/admin*", "/admin"));
        assert!(!matches_pattern("/admin*", "/other"));
        assert!(matches_pattern("/*.json$", "/data.json"));
        assert!(!matches_pattern("/*.json$", "/data.json?q=1"));
    }

    #[test]
    fn test_longest_match_wins() {
        let rules = vec![
            RobotsRule { pattern: "/".to_string(), allow: false },
            RobotsRule { pattern: "/allowed/".to_string(), allow: true },
        ];
        let rule = match_path_to_rule("/allowed/page", &rules).unwrap();
        assert!(rule.allow);
    }
}
