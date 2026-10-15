//! Patterns: a value that changes from one step of a cycle to the next,
//! written between quotes as in Strudel's mini-notation, and the words
//! that change the time an effect sees (`fast`, `slow`, `rev`, `every`).
//!
//! ```text
//! rotate="0 40 80"          three steps in a cycle
//! rotate="0 [40 80]"        the second step holds two, twice as short
//! rotate="0 40 ~ 80"        ~ is a rest: the parameter's default
//! rotate="<0 40 80>"        one value a cycle, in turn
//! rotate="0 40*2 80/2"      a step twice as fast, a step twice as slow
//! every 4 rev spin angle="5 20"
//! ```
//!
//! A step is any text a parameter can read: a number, a colour, `true`,
//! `lighten`, even a range `0..40` or a pair `10,50`.

use std::str::FromStr;

/// Where in a pattern an effect is read: where its ranges stand (`a..b`,
/// 0 to 1), and how many cycles have gone by, for its patterns.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Time {
    pub moment: f64,
    pub cycle: f64,
}

impl Time {
    pub fn new(moment: f64, cycle: f64) -> Time {
        Time { moment, cycle }
    }
}

/// How far from the end of a cycle a reversed time stays, so that the
/// start of a step does not fall in the next one.
const EDGE: f64 = 1e-9;

/// A change of the cycle an effect sees, written before its name.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// `fast 2`: twice as many cycles go by.
    Fast(f64),
    /// `slow 2`: half as many cycles go by.
    Slow(f64),
    /// `rev`: every cycle runs backwards.
    Rev,
    /// `every 4 rev`: the change on every 4th cycle, the first included.
    Every(u32, Box<Change>),
}

impl Change {
    /// The cycle this change makes of `cycle`.
    pub fn apply(&self, cycle: f64) -> f64 {
        match self {
            Change::Fast(k) => cycle * k,
            Change::Slow(k) => cycle / k,
            Change::Rev => {
                let whole = cycle.floor();
                whole + (1.0 - (cycle - whole) - EDGE).max(0.0)
            }
            Change::Every(n, change) => {
                if (cycle.floor() as i64).rem_euclid(i64::from(*n)) == 0 {
                    change.apply(cycle)
                } else {
                    cycle
                }
            }
        }
    }

    /// The change that starts at `words[at]`, and how many words it takes:
    /// `None` when the word is not the start of one. A mistake in its
    /// arguments is an error.
    pub fn read<S: AsRef<str>>(words: &[S], at: usize) -> Result<Option<(Change, usize)>, String> {
        let word = |i: usize| words.get(i).map(AsRef::as_ref);
        let number = |i: usize, what: &str| -> Result<f64, String> {
            word(i)
                .and_then(|text| text.parse::<f64>().ok())
                .filter(|n| *n > 0.0 && n.is_finite())
                .ok_or_else(|| format!("{what} needs a number above 0"))
        };
        Ok(match word(at) {
            Some("fast") => Some((Change::Fast(number(at + 1, "fast")?), 2)),
            Some("slow") => Some((Change::Slow(number(at + 1, "slow")?), 2)),
            Some("rev") => Some((Change::Rev, 1)),
            Some("every") => {
                let n = word(at + 1)
                    .and_then(|text| text.parse::<u32>().ok())
                    .filter(|n| *n > 0)
                    .ok_or("every needs a whole number above 0, then a change (rev, fast 2…)")?;
                let (change, used) = Change::read(words, at + 2)?
                    .ok_or("every needs a change after its number (rev, fast 2, slow 2)")?;
                Some((Change::Every(n, Box::new(change)), 2 + used))
            }
            _ => None,
        })
    }
}

/// The words of a recipe: split at blanks, except inside double quotes,
/// which stay in the word, so that a pattern is one word.
pub fn split_words(text: &str) -> Vec<String> {
    let (mut words, mut word, mut quoted) = (Vec::new(), String::new(), false);
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                word.push(c);
            }
            c if c.is_whitespace() && !quoted => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            c => word.push(c),
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// What a step of a pattern is.
#[derive(Debug, Clone, PartialEq)]
enum Node {
    /// A value, as text.
    Atom(String),
    /// Nothing: the parameter's default.
    Rest,
    /// Steps sharing a cycle equally.
    Sequence(Vec<Node>),
    /// One step a cycle, in turn.
    Alternate(Vec<Node>),
    /// The step run `factor` times as fast.
    Scaled(Box<Node>, f64),
}

impl Node {
    /// The value at a time in cycles, from 0.
    fn at(&self, time: f64) -> Option<&str> {
        match self {
            Node::Atom(text) => Some(text),
            Node::Rest => None,
            Node::Sequence(steps) => {
                let count = steps.len() as f64;
                let whole = time.floor();
                let place = (time - whole) * count;
                let index = (place.floor() as usize).min(steps.len().saturating_sub(1));
                // A step has its own cycles: the cycle number goes on, the
                // place in the step is where the time is within it.
                steps.get(index)?.at(whole + (place - index as f64))
            }
            Node::Alternate(steps) => {
                let count = steps.len() as i64;
                let whole = time.floor() as i64;
                let index = whole.rem_euclid(count.max(1)) as usize;
                // Inside, a cycle goes by for each time all the steps were
                // played: a step that alternates itself advances then.
                steps
                    .get(index)?
                    .at(whole.div_euclid(count.max(1)) as f64 + (time - time.floor()))
            }
            Node::Scaled(step, factor) => step.at(time * factor),
        }
    }

    fn atoms<'a>(&'a self, into: &mut Vec<&'a str>) {
        match self {
            Node::Atom(text) => into.push(text),
            Node::Rest => {}
            Node::Sequence(steps) | Node::Alternate(steps) => {
                steps.iter().for_each(|step| step.atoms(into));
            }
            Node::Scaled(step, _) => step.atoms(into),
        }
    }
}

/// A pattern, read from text between double quotes.
#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    root: Node,
}

impl Pattern {
    /// Whether a parameter's text is a pattern: it begins with a quote.
    pub fn is_pattern(text: &str) -> bool {
        text.starts_with('"')
    }

    /// The value at a time in cycles, `None` for a rest.
    pub fn at(&self, cycle: f64) -> Option<&str> {
        self.root.at(cycle)
    }

    /// Every value the pattern can give, to check them all.
    pub fn atoms(&self) -> Vec<&str> {
        let mut atoms = Vec::new();
        self.root.atoms(&mut atoms);
        atoms
    }
}

impl FromStr for Pattern {
    type Err = String;

    fn from_str(text: &str) -> Result<Pattern, String> {
        let inner = text
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
            .filter(|rest| !rest.contains('"'))
            .ok_or("a pattern is written between double quotes")?;
        let mut reader = Reader {
            chars: inner.chars().collect(),
            at: 0,
        };
        let steps = reader.sequence(None)?;
        if steps.is_empty() {
            return Err("a pattern needs at least one step".to_owned());
        }
        Ok(Pattern {
            root: Node::Sequence(steps),
        })
    }
}

struct Reader {
    chars: Vec<char>,
    at: usize,
}

impl Reader {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn skip_blanks(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.at += 1;
        }
    }

    /// Steps up to the `close` that ends them, or the end of the text.
    fn sequence(&mut self, close: Option<char>) -> Result<Vec<Node>, String> {
        let mut steps = Vec::new();
        loop {
            self.skip_blanks();
            match (self.peek(), close) {
                (None, None) => return Ok(steps),
                (None, Some(close)) => return Err(format!("a '{close}' is missing")),
                (Some(c), Some(close)) if c == close => {
                    self.at += 1;
                    return Ok(steps);
                }
                (Some(c @ (']' | '>')), _) => return Err(format!("a '{c}' with nothing to close")),
                _ => steps.push(self.step()?),
            }
        }
    }

    /// One step with the `*n` or `/n` after it, if any.
    fn step(&mut self) -> Result<Node, String> {
        let mut node = match self.peek() {
            Some('[') => {
                self.at += 1;
                Node::Sequence(self.sequence(Some(']'))?)
            }
            Some('<') => {
                self.at += 1;
                Node::Alternate(self.sequence(Some('>'))?)
            }
            _ => self.atom()?,
        };
        if let Node::Sequence(steps) | Node::Alternate(steps) = &node
            && steps.is_empty()
        {
            return Err("empty brackets".to_owned());
        }
        while let Some(sign @ ('*' | '/')) = self.peek() {
            self.at += 1;
            let start = self.at;
            while self.peek().is_some_and(|c| c.is_ascii_digit() || c == '.') {
                self.at += 1;
            }
            let digits: String = self.chars[start..self.at].iter().collect();
            let factor: f64 = digits
                .parse()
                .ok()
                .filter(|n: &f64| *n > 0.0 && n.is_finite())
                .ok_or_else(|| format!("'{sign}' needs a number above 0"))?;
            node = Node::Scaled(
                Box::new(node),
                if sign == '*' { factor } else { 1.0 / factor },
            );
        }
        Ok(node)
    }

    fn atom(&mut self) -> Result<Node, String> {
        let start = self.at;
        while self
            .peek()
            .is_some_and(|c| !c.is_whitespace() && !"[]<>*/".contains(c))
        {
            self.at += 1;
        }
        let text: String = self.chars[start..self.at].iter().collect();
        match text.as_str() {
            "" => Err(format!(
                "a step is missing before '{}'",
                self.peek().unwrap_or(' ')
            )),
            "~" => Ok(Node::Rest),
            _ => Ok(Node::Atom(text)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(text: &str) -> Pattern {
        format!("\"{text}\"").parse().unwrap()
    }

    /// The values over the start of each of `slots` equal parts of cycle
    /// `cycle`.
    fn run(text: &str, cycle: u32, slots: u32) -> Vec<String> {
        let pattern = pattern(text);
        (0..slots)
            .map(|i| {
                let time = f64::from(cycle) + (f64::from(i) + 0.5) / f64::from(slots);
                pattern.at(time).unwrap_or("~").to_owned()
            })
            .collect()
    }

    #[test]
    fn a_sequence_shares_the_cycle_and_comes_round() {
        assert_eq!(run("0 40 80", 0, 3), ["0", "40", "80"]);
        assert_eq!(run("0 40 80", 7, 3), ["0", "40", "80"]);
        assert_eq!(pattern("0 40 80").at(0.0), Some("0"));
        assert_eq!(
            pattern("0 40 80").at(1.0 / 3.0),
            Some("40"),
            "a step starts on its boundary"
        );
        assert_eq!(pattern("0 40 80").at(2.999), Some("80"));
        assert_eq!(pattern("0 40 80").at(3.0), Some("0"));
    }

    #[test]
    fn brackets_rests_and_speeds() {
        assert_eq!(run("0 [40 80]", 0, 4), ["0", "0", "40", "80"]);
        assert_eq!(run("0 ~ 80", 0, 3), ["0", "~", "80"]);
        assert_eq!(run("0 40*2", 0, 4), ["0", "0", "40", "40"]);
        assert_eq!(
            run("0 [40 50]/2", 0, 2),
            ["0", "40"],
            "half of the inner cycle"
        );
        assert_eq!(
            run("0 [40 50]/2", 1, 2),
            ["0", "50"],
            "the other half next cycle"
        );
        assert_eq!(run("[0 1]*2", 0, 4), ["0", "1", "0", "1"]);
    }

    #[test]
    fn angle_brackets_play_one_step_a_cycle() {
        assert_eq!(run("<0 40 80>", 0, 1), ["0"]);
        assert_eq!(run("<0 40 80>", 1, 1), ["40"]);
        assert_eq!(run("<0 40 80>", 5, 1), ["80"]);
        // Inside a sequence, and one alternation inside another.
        assert_eq!(run("a <b c>", 0, 2), ["a", "b"]);
        assert_eq!(run("a <b c>", 1, 2), ["a", "c"]);
        assert_eq!(run("<a <b c>>", 0, 1), ["a"]);
        assert_eq!(run("<a <b c>>", 1, 1), ["b"]);
        assert_eq!(run("<a <b c>>", 3, 1), ["c"]);
    }

    #[test]
    fn steps_can_be_colours_pairs_and_ranges() {
        assert_eq!(
            run("ff0000 #00f 10,50 0..40", 0, 4),
            ["ff0000", "#00f", "10,50", "0..40"]
        );
        assert_eq!(pattern("0 40 ~").atoms(), ["0", "40"]);
    }

    #[test]
    fn mistakes_are_named() {
        for bad in [
            "",
            "0 40",
            "\"0 [40\"",
            "\"0 40]\"",
            "\"\"",
            "\"0 *2\"",
            "\"0 40*\"",
            "\"[]\"",
            "\"0*0\"",
        ] {
            assert!(bad.parse::<Pattern>().is_err(), "{bad:?}");
        }
        assert!(
            "\"0 [40\""
                .parse::<Pattern>()
                .unwrap_err()
                .contains("missing")
        );
    }

    #[test]
    fn a_reversed_cycle_runs_backwards_step_by_step() {
        let reversed = |time: f64| Change::Rev.apply(time);
        let pattern = pattern("0 40 80");
        let at = |time: f64| pattern.at(reversed(time)).unwrap().to_owned();
        assert_eq!(
            [0.0, 0.2, 1.0 / 3.0, 0.5, 2.0 / 3.0, 0.9].map(at),
            ["80", "80", "40", "40", "0", "0"]
        );
        // A reversed cycle stays in its cycle, the first moment included.
        assert!(reversed(2.0) > 2.99 && reversed(2.0) < 3.0);
        assert!((reversed(2.999999) - 2.0).abs() < 1e-3);
    }

    #[test]
    fn fast_slow_and_every_change_the_cycle() {
        assert_eq!(Change::Fast(2.0).apply(1.25), 2.5);
        assert_eq!(Change::Slow(2.0).apply(1.0), 0.5);
        let every = Change::Every(3, Box::new(Change::Fast(2.0)));
        assert_eq!(every.apply(0.25), 0.5, "the first cycle");
        assert_eq!(every.apply(1.25), 1.25);
        assert_eq!(every.apply(2.25), 2.25);
        assert_eq!(every.apply(3.25), 6.5, "the 4th: cycle 3");
    }

    #[test]
    fn changes_are_read_before_an_effect() {
        let words = ["every", "4", "rev", "spin", "angle=5"];
        assert_eq!(
            Change::read(&words, 0),
            Ok(Some((Change::Every(4, Box::new(Change::Rev)), 3)))
        );
        assert_eq!(Change::read(&words, 3), Ok(None));
        assert_eq!(
            Change::read(&["fast", "2"], 0),
            Ok(Some((Change::Fast(2.0), 2)))
        );
        assert!(Change::read(&["fast"], 0).is_err());
        assert!(Change::read(&["fast", "0"], 0).is_err());
        assert!(Change::read(&["every", "4"], 0).is_err());
        assert!(Change::read(&["every", "x", "rev"], 0).is_err());
    }

    #[test]
    fn words_split_at_blanks_but_not_inside_quotes() {
        assert_eq!(
            split_words("seed=7 rotate=\"0 40  80\"\n+ fade   lift=3"),
            ["seed=7", "rotate=\"0 40  80\"", "+", "fade", "lift=3"]
        );
        assert_eq!(split_words("a=\"unfinished b"), ["a=\"unfinished b"]);
        assert!(split_words("  \n ").is_empty());
    }
}
