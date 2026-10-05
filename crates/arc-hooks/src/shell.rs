use std::iter::Peekable;
use std::str::Chars;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Indent {
    Kept,
    Stripped,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Target {
    #[default]
    Command,
    File,
    Heredoc(Indent),
}

struct Heredoc {
    delimiter: String,
    indent: Indent,
}

#[derive(Default)]
struct Splitter {
    commands: Vec<Vec<String>>,
    words: Vec<String>,
    word: Option<String>,
    target: Target,
    heredocs: Vec<Heredoc>,
}

impl Splitter {
    fn word(&mut self) -> &mut String {
        self.word.get_or_insert_with(String::new)
    }

    fn finish_word(&mut self) {
        if let Some(word) = self.word.take() {
            match std::mem::take(&mut self.target) {
                Target::Command => self.words.push(word),
                Target::File => {}
                Target::Heredoc(indent) => self.heredocs.push(Heredoc {
                    delimiter: word,
                    indent,
                }),
            }
        }
    }

    fn finish_command(&mut self) {
        self.finish_word();
        self.target = Target::Command;
        if !self.words.is_empty() {
            self.commands.push(std::mem::take(&mut self.words));
        }
    }

    fn redirect(&mut self, first: char, chars: &mut Peekable<Chars>) {
        match self.word.as_deref() {
            Some(fd) if fd.chars().all(|c| c.is_ascii_digit()) => self.word = None,
            _ => self.finish_word(),
        }
        let mut operator = String::from(first);
        while let Some(c) = chars.next_if(|&c| matches!(c, '>' | '<' | '&' | '|')) {
            operator.push(c);
        }
        self.target = match operator.as_str() {
            "<<" => Target::Heredoc(match chars.next_if_eq(&'-') {
                Some(_) => Indent::Stripped,
                None => Indent::Kept,
            }),
            _ => Target::File,
        };
    }

    fn skip_heredocs(&mut self, chars: &mut Peekable<Chars>) {
        for doc in std::mem::take(&mut self.heredocs) {
            while chars.peek().is_some() {
                let body: String = chars.by_ref().take_while(|&c| c != '\n').collect();
                let line = match doc.indent {
                    Indent::Stripped => body.trim_start_matches('\t'),
                    Indent::Kept => body.as_str(),
                };
                if line == doc.delimiter {
                    break;
                }
            }
        }
    }

    fn single_quoted(&mut self, chars: &mut Peekable<Chars>) {
        let word = self.word();
        for c in chars.by_ref() {
            match c {
                '\'' => return,
                c => word.push(c),
            }
        }
    }

    fn double_quoted(&mut self, chars: &mut Peekable<Chars>) {
        let word = self.word();
        while let Some(c) = chars.next() {
            match c {
                '"' => return,
                '\\' => match chars.next_if(|&c| matches!(c, '"' | '\\' | '$' | '`' | '\n')) {
                    Some('\n') => {}
                    Some(escaped) => word.push(escaped),
                    None => word.push('\\'),
                },
                c => word.push(c),
            }
        }
    }

    fn literal_until(&mut self, chars: &mut Peekable<Chars>, open: char, close: char) {
        let word = self.word();
        let mut depth = 1usize;
        for c in chars.by_ref() {
            word.push(c);
            if c == close {
                depth -= 1;
                if depth == 0 {
                    return;
                }
            } else if c == open && open != close {
                depth += 1;
            }
        }
    }
}

pub fn simple_commands(script: &str) -> Vec<Vec<String>> {
    let mut split = Splitter::default();
    let mut chars = script.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => split.finish_word(),
            '\n' => {
                split.finish_command();
                split.skip_heredocs(&mut chars);
            }
            ';' | '&' | '|' | '(' | ')' => split.finish_command(),
            '#' if split.word.is_none() => while chars.next_if(|&c| c != '\n').is_some() {},
            '\'' => split.single_quoted(&mut chars),
            '"' => split.double_quoted(&mut chars),
            '\\' => match chars.next() {
                Some('\n') | None => {}
                Some(escaped) => split.word().push(escaped),
            },
            '>' | '<' => split.redirect(c, &mut chars),
            '$' if chars.peek() == Some(&'(') => {
                split.word().push('$');
                chars.next();
                split.word().push('(');
                split.literal_until(&mut chars, '(', ')');
            }
            '`' => {
                split.word().push('`');
                split.literal_until(&mut chars, '`', '`');
            }
            c => split.word().push(c),
        }
    }
    split.finish_command();
    split.commands
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(script: &str) -> Vec<Vec<&'static str>> {
        simple_commands(script)
            .into_iter()
            .map(|words| {
                words
                    .into_iter()
                    .map(|w| &*Box::leak(w.into_boxed_str()))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn splits_words_and_command_separators() {
        assert_eq!(
            split("cd repo && git checkout -b x; git log | head\nls &"),
            vec![
                vec!["cd", "repo"],
                vec!["git", "checkout", "-b", "x"],
                vec!["git", "log"],
                vec!["head"],
                vec!["ls"],
            ]
        );
        assert_eq!(split("  "), Vec::<Vec<&str>>::new());
        assert_eq!(
            split("(cd a || exit) && b"),
            vec![vec!["cd", "a"], vec!["exit"], vec!["b"]]
        );
    }

    #[test]
    fn quotes_and_escapes() {
        assert_eq!(
            split(r#"git switch -c 'my branch' "a \"b\" $x" c\ d '' e"#),
            vec![vec![
                "git",
                "switch",
                "-c",
                "my branch",
                r#"a "b" $x"#,
                "c d",
                "",
                "e"
            ]]
        );
        assert_eq!(
            split("git \\\n  branch x"),
            vec![vec!["git", "branch", "x"]]
        );
        assert_eq!(
            split("echo 'unterminated"),
            vec![vec!["echo", "unterminated"]]
        );
    }

    #[test]
    fn drops_redirections_and_comments() {
        assert_eq!(
            split("git checkout -b x 2>&1 >/dev/null # make x\ngit branch y > out.txt"),
            vec![
                vec!["git", "checkout", "-b", "x"],
                vec!["git", "branch", "y"]
            ]
        );
        assert_eq!(split("a#b"), vec![vec!["a#b"]]);
    }

    #[test]
    fn heredoc_bodies_are_not_commands() {
        assert_eq!(
            split("cat <<EOF > f\ngit checkout -b x\n EOF\nEOF\ngit branch y"),
            vec![vec!["cat"], vec!["git", "branch", "y"]]
        );
        assert_eq!(
            split("cat <<-'END'; echo a\n\t\tgit branch x\n\tEND\nls"),
            vec![vec!["cat"], vec!["echo", "a"], vec!["ls"]]
        );
        assert_eq!(
            split("a <<\"ONE\" && b << TWO\nx\nONE\ny\nTWO\nc"),
            vec![vec!["a"], vec!["b"], vec!["c"]]
        );
        assert_eq!(
            split("git commit -F- <<MSG\nunterminated\ngit branch z"),
            vec![vec!["git", "commit", "-F-"]]
        );
        assert_eq!(
            split("grep x <<< 'git branch y'\ngit branch z"),
            vec![vec!["grep", "x"], vec!["git", "branch", "z"]]
        );
    }

    #[test]
    fn substitutions_stay_one_word() {
        assert_eq!(
            split("git branch $(echo a b) `x y`"),
            vec![vec!["git", "branch", "$(echo a b)", "`x y`"]]
        );
    }
}
