use std::future::Future;
use std::pin::Pin;

use crate::plugin::{Meta, Plugin};
use crate::system::icon::resolve;
use crate::wire::ResultItem;
use anyhow::Result;
use rust_i18n::t;

pub struct Calculator {
    meta: Meta,
}

impl Calculator {
    pub fn new() -> Self {
        Self {
            meta: Meta {
                id: "calculator",
                name: t!("plugin.calculator.name"),
                icon: "builtin:calculator",
                ready: t!("plugin.calculator.ready"),
            },
        }
    }
}

impl Plugin for Calculator {
    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn search(
        &self,
        _query: &str,
        full: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ResultItem>>> + Send + '_>> {
        let expr = full.to_string();
        Box::pin(async move { Ok(do_search(&expr)) })
    }
}

fn do_search(expr: &str) -> Vec<ResultItem> {
    if expr.is_empty() {
        return vec![];
    }

    // fasteval ships no constants or math built-ins; provide meval's surface
    // (pi/e/tau + trig family). Identifiers fold to lowercase, PI == pi.
    let mut ns = |name: &str, args: Vec<f64>| -> Option<f64> {
        let name = name.to_ascii_lowercase();
        let one = |f: fn(f64) -> f64| args.first().map(|&v| f(v));
        match name.as_str() {
            // one-argument functions
            "sqrt" => one(f64::sqrt),
            "cbrt" => one(f64::cbrt),
            "exp" => one(f64::exp),
            "ln" => one(f64::ln),
            "log" => one(f64::log10),
            "log2" => one(f64::log2),
            "sin" => one(f64::sin),
            "cos" => one(f64::cos),
            "tan" => one(f64::tan),
            "asin" => one(f64::asin),
            "acos" => one(f64::acos),
            "atan" => one(f64::atan),
            "sinh" => one(f64::sinh),
            "cosh" => one(f64::cosh),
            "tanh" => one(f64::tanh),
            "abs" => one(f64::abs),
            "floor" => one(f64::floor),
            "ceil" => one(f64::ceil),
            "round" => one(f64::round),
            // two-argument functions
            "pow" | "atan2" | "hypot" | "min" | "max" => {
                if args.len() == 2 {
                    let (a, b) = (args[0], args[1]);
                    Some(match name.as_str() {
                        "pow" => a.powf(b),
                        "atan2" => a.atan2(b),
                        "hypot" => a.hypot(b),
                        "min" => a.min(b),
                        _ => a.max(b),
                    })
                } else {
                    None
                }
            }
            _ if args.is_empty() => match name.as_str() {
                "pi" => Some(std::f64::consts::PI),
                "tau" => Some(std::f64::consts::TAU),
                "e" => Some(std::f64::consts::E),
                _ => None,
            },
            _ => None,
        }
    };

    match fasteval::ez_eval(expr, &mut ns) {
        Ok(value) => {
            if value.is_infinite() || value.is_nan() {
                return vec![];
            }
            let formatted = if value.fract() == 0.0 {
                format!("{}", value as i64)
            } else {
                format!("{value:.10}")
                    .trim_end_matches('0')
                    .trim_end_matches('.')
                    .to_string()
            };

            vec![ResultItem {
                title: formatted,
                summary: Some(expr.to_string()),
                on_click: None,
                icon: resolve("builtin:calculator"),
                ephemeral: true,
                actions: Vec::new(),
                badge: None,
            }]
        }
        Err(_) => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first(expr: &str) -> String {
        do_search(expr)
            .first()
            .map(|r| r.title.clone())
            .unwrap_or_default()
    }

    #[test]
    fn plain_arithmetic_is_evaluated() {
        assert_eq!(first("2 + 3"), "5");
        assert_eq!(first("10 - 7"), "3");
        assert_eq!(first("6 * 4"), "24");
        assert_eq!(first("15 / 3"), "5");
    }

    #[test]
    fn empty_input_is_not_math() {
        assert!(do_search("").is_empty());
    }

    #[test]
    fn a_computed_answer_is_ephemeral() {
        assert!(do_search("2 + 3")[0].ephemeral);
    }

    #[test]
    fn division_by_zero_is_not_an_answer() {
        assert!(do_search("1/0").is_empty());
    }

    #[test]
    fn prose_is_not_math() {
        assert!(do_search("firefox").is_empty());
        assert!(do_search("hello world").is_empty());
    }

    #[test]
    fn a_decimal_result_is_kept() {
        let result = first("3.14 * 2");
        assert!(result.starts_with("6.28"));
    }

    #[test]
    fn a_negative_result_is_kept() {
        assert_eq!(first("-5 + 8"), "3");
        assert_eq!(first("0 - 10"), "-10");
    }

    #[test]
    fn a_power_expression_is_evaluated() {
        assert_eq!(first("2 ^ 10"), "1024");
    }

    #[test]
    fn named_constants_are_evaluated() {
        let pi = first("pi");
        assert!(pi.starts_with("3.14159"));
        assert!(first("PI").starts_with("3.14159"));
        let e = first("e ^ 2");
        assert!(e.starts_with("7.3890"));
    }

    #[test]
    fn math_functions_are_evaluated() {
        let half = first("sin(pi / 2)");
        assert!(half.starts_with('1'));
        assert_eq!(first("abs(-5)"), "5");
        assert_eq!(first("cos(0)"), "1");
        assert!(first("log(1000)").starts_with('3'));
        assert_eq!(first("floor(2.9)"), "2");
        assert_eq!(first("ceil(2.1)"), "3");
        assert_eq!(first("min(3, 8)"), "3");
        assert_eq!(first("max(3, 8)"), "8");
    }

    #[test]
    fn unknown_identifier_is_not_math() {
        assert!(do_search("bottles").is_empty());
    }
}
