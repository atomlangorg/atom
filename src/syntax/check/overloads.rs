/*
 * Copyright 2026 George Elsham
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Choosing between the methods or static functions of a type that share a
//! name, which are told apart by their labels, parameter types and return
//! types, where `_` counts as a label.
//!
//! A call means the one whose labels are its arguments', whose parameters are
//! its arguments' types, and, if that still leaves more than one, which
//! returns what the call is used as. Functions that are the same in all of
//! those can only share a name if they're in `impl`s with constraints, and
//! the one from the more specialised `impl`, which has constraints, is
//! chosen. If more than one specialised function could be meant, the call is
//! ambiguous.

use super::*;

/// A call to one of the functions that share a name.
#[derive(Clone, Copy)]
pub(super) struct OverloadCall<'e> {
    pub(super) args: &'e [Arg],
    /// The types of `args`, if they've already been checked.
    pub(super) found: Option<&'e [Option<Type>]>,
    /// What the call is used as, if that's known.
    pub(super) expected: Option<&'e Type>,
    pub(super) span: Span,
}

impl<'a> Checker<'a> {
    /// The method of `ty` called `name`, or its static function if
    /// `is_static`, that `call` calls, which `ty` must have at least one of.
    /// Returns `None` if there isn't exactly one it could be, which has been
    /// reported, along with the errors of its arguments.
    ///
    /// When there's only one it could be by its labels, it's that one, so
    /// the call's mistakes are reported as they would be for any other call.
    pub(super) fn overload(
        &mut self,
        ty: &Type,
        name: &str,
        is_static: bool,
        call: OverloadCall,
    ) -> Option<FnRef> {
        let OverloadCall {
            args, found, span, ..
        } = call;
        let met = self.methods(ty, name, is_static);
        let mut declared = self.declared_methods(ty, name, is_static);
        if declared.is_empty() {
            declared.clone_from(&met);
        }
        let labelled: Vec<_> = declared
            .into_iter()
            .filter(|function| self.has_labels(function, args))
            .collect();
        let met_labelled: Vec<_> = labelled
            .iter()
            .filter(|function| met.contains(function))
            .cloned()
            .collect();
        let shown = format!("{ty}.{name}");
        let candidates = match &met_labelled[..] {
            [function] => return Some(function.clone()),
            [] => {
                // One with these labels, or any if there are none, may only
                // be for types that meet its constraints.
                let unmet = labelled
                    .iter()
                    .find_map(|declared| self.unmet(ty, name, declared))
                    .or_else(|| {
                        met.is_empty()
                            .then(|| self.unmet_methods(ty, name, is_static))
                            .flatten()
                    });
                if let Some(error) = unmet {
                    self.error(span, error);
                    self.check_unchosen(found, args);
                    return None;
                }
                if let [function] = &met[..] {
                    return Some(function.clone());
                }
                // None of them can be meant without the call's labels.
                self.check_unchosen(found, args);
                self.no_matching_overload(&shown, name, &met, span);
                return None;
            }
            _ => met_labelled,
        };

        let mut matching = vec![];
        for function in &candidates {
            let signature = self.signature_of(function, span);
            let params: Vec<_> = signature
                .params
                .iter()
                .map(|param| param.ty.clone())
                .collect();
            let fits = match found {
                Some(found) => fits(found, &params),
                // Each argument takes its type from its parameter, so they're
                // checked as each function's arguments.
                None => {
                    let ((found, _), failed) = self.trial(|checker| checker.args(args, &params));
                    !failed && fits(&found, &params)
                }
            };
            if fits {
                matching.push((function.clone(), signature));
            }
        }
        if matching.is_empty() {
            // The arguments' own mistakes may be why.
            let start = self.errors.len();
            self.check_unchosen(found, args);
            if self.errors.len() == start {
                self.no_matching_overload(&shown, name, &candidates, span);
            }
            return None;
        }
        if let Some(expected) = call.expected
            && matching.len() > 1
            && matching
                .iter()
                .any(|(_, signature)| signature.returns.as_ref() == Some(expected))
        {
            matching.retain(|(_, signature)| signature.returns.as_ref() == Some(expected));
        }
        self.most_specialised(name, shown, matching, span)
    }
    /// Whether `function`'s parameters have the labels of `args`, in order.
    pub(super) fn has_labels(&self, function: &FnRef, args: &[Arg]) -> bool {
        let params = &self.functions[function.index].params;
        params
            .iter()
            .map(|param| &param.label)
            .eq(args.iter().map(|arg| &arg.label))
    }
    /// The static function of `ty` called `name` used as a value that's
    /// expected to be `expected`, at `span`, which `ty` must have at least
    /// one of. Returns `None` if there isn't exactly one it could be, which
    /// has been reported.
    pub(super) fn function_as_value(
        &mut self,
        ty: &Type,
        name: &str,
        expected: Option<&Type>,
        span: Span,
    ) -> Option<FnRef> {
        let met = self.methods(ty, name, true);
        if let [function] = &met[..] {
            return Some(function.clone());
        }
        let mut matching: Vec<_> = met
            .into_iter()
            .map(|function| {
                let signature = self.signature_of(&function, span);
                (function, signature)
            })
            .collect();
        if let Some(expected) = expected {
            let mut typed = vec![];
            for (function, signature) in &matching {
                if self.function_type(function, span).as_ref() == Some(expected) {
                    typed.push((function.clone(), signature.clone()));
                }
            }
            if !typed.is_empty() {
                matching = typed;
            }
        }
        self.most_specialised(name, format!("{ty}.{name}"), matching, span)
    }
    /// The only one of `matching`, the functions called `name` that something
    /// could mean, each with its signature, or the one from the most
    /// specialised `impl` if they all have the same signature. Otherwise,
    /// reports that what's shown as `shown` at `span` is ambiguous.
    fn most_specialised(
        &mut self,
        name: &str,
        shown: String,
        matching: Vec<(FnRef, Signature<'a>)>,
        span: Span,
    ) -> Option<FnRef> {
        if let [(function, _)] = &matching[..] {
            return Some(function.clone());
        }
        let (_, first) = &matching[0];
        if matching
            .iter()
            .all(|(_, signature)| same_signature(signature, first))
        {
            let specialised: Vec<_> = matching
                .iter()
                .filter(|(function, _)| self.functions[function.index].constraints.is_some())
                .collect();
            if let [(function, _)] = specialised[..] {
                return Some(function.clone());
            }
        }
        let void = self.std_type("Void");
        let candidates = matching
            .iter()
            .map(|(_, signature)| signature_text(name, signature, &void))
            .collect();
        let error = CheckError::AmbiguousOverload {
            function: shown,
            candidates,
        };
        self.error(span, error);
        None
    }
    /// Reports that no function called `name` that what's shown as `shown`
    /// at `span` could mean, which are `candidates`, takes its arguments.
    fn no_matching_overload(&mut self, shown: &str, name: &str, candidates: &[FnRef], span: Span) {
        let void = self.std_type("Void");
        let candidates = candidates
            .iter()
            .map(|function| {
                let signature = self.signature_of(function, span);
                signature_text(name, &signature, &void)
            })
            .collect();
        let error = CheckError::NoMatchingOverload {
            function: shown.to_string(),
            candidates,
        };
        self.error(span, error);
    }
    /// Checks `args` for their own errors, unless they've already been
    /// checked, which is when `found` is given, since none of the functions
    /// they could be for were chosen.
    fn check_unchosen(&mut self, found: Option<&[Option<Type>]>, args: &[Arg]) {
        if found.is_none() {
            self.args(args, &[]);
        }
    }
    /// Runs `check` to see what it would find, then forgets the errors it
    /// found, returning what it returns and whether it found any. Errors that
    /// would only be found once are still reported, as `Checker::kept` says.
    pub(super) fn trial<T>(&mut self, check: impl FnOnce(&mut Self) -> T) -> (T, bool) {
        let start = self.errors.len();
        let evaluated_never = self.evaluated_never;
        self.trials += 1;
        let checked = check(self);
        self.trials -= 1;
        let failed = self.errors.len() > start;
        self.errors.truncate(start);
        self.evaluated_never = evaluated_never;
        if self.trials == 0 {
            let kept = std::mem::take(&mut self.kept);
            self.errors.extend(kept);
        }
        (checked, failed)
    }
    /// Keeps the errors found since there were `start`, which would only be
    /// found once, if they were found while trying something with `trial`.
    pub(super) fn keep_errors(&mut self, start: usize) {
        if self.trials > 0 {
            let found: Vec<_> = self.errors.drain(start..).collect();
            self.kept.extend(found);
        }
    }
}

/// Whether arguments of types `found` can be given to parameters of types
/// `params`. An unknown type has already been reported, so it fits.
fn fits(found: &[Option<Type>], params: &[Option<Type>]) -> bool {
    found.len() == params.len()
        && found.iter().zip(params).all(|pair| match pair {
            (Some(found), Some(param)) => found == param,
            _ => true,
        })
}

/// Whether `a` and `b` have the same labels, parameter types and return type,
/// which are all known.
pub(super) fn same_signature(a: &Signature, b: &Signature) -> bool {
    let known = |ty: &Option<Type>| ty.is_some();
    a.params.len() == b.params.len()
        && a.params
            .iter()
            .zip(&b.params)
            .all(|(a, b)| a.label == b.label && known(&a.ty) && a.ty == b.ty)
        && known(&a.returns)
        && a.returns == b.returns
}

/// How a function called `name` with `signature` is shown in messages, as in
/// `fn new(_ value: Void) -> Shape`.
fn signature_text(name: &str, signature: &Signature, void: &Type) -> String {
    let params: Vec<_> = signature
        .params
        .iter()
        .map(|param| {
            let ty = param
                .ty
                .as_ref()
                .map_or("_".to_string(), ToString::to_string);
            let element = ast::TupleElement {
                label: param.label.clone(),
                name: param.name.map(str::to_string),
                ty: TypeName::new(ty),
                span: Span::default(),
            };
            element.to_string()
        })
        .collect();
    let returns = match &signature.returns {
        Some(returns) if returns != void => format!(" -> {returns}"),
        _ => String::new(),
    };
    format!("fn {name}({}){returns}", params.join(", "))
}
