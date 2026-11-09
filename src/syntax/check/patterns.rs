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

//! Checking patterns, and that a `match`'s patterns match every value.

use super::*;

/// Which values a pattern matches, for checking that a `match`'s patterns
/// match every value, and that each matches something new.
#[derive(Debug, Clone)]
pub(super) enum Space {
    Any,
    /// The values made with `Ctor`, whose parts match each of these in turn.
    Ctor(Ctor, Vec<Space>),
}

/// A way of making a value that patterns can tell apart from the others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Ctor {
    Int(i128),
    Bool(bool),
    /// A tuple, whose parts are its elements.
    Tuple,
    /// The option of a choice at this index, whose part is its value.
    Option(usize),
}

/// The names of the values that `pattern` binds, in order.
pub(super) fn bindings(pattern: &Pattern) -> Vec<(&str, Span)> {
    match &pattern.kind {
        PatternKind::Binding(name) => vec![(name, pattern.span)],
        PatternKind::Tuple(elements) => elements
            .iter()
            .flat_map(|element| bindings(&element.pattern))
            .collect(),
        PatternKind::Option {
            value: Some(value), ..
        } => bindings(value),
        PatternKind::Any
        | PatternKind::Int { .. }
        | PatternKind::Bool(_)
        | PatternKind::Option { .. } => vec![],
    }
}

impl<'a> Checker<'a> {
    /// Checks `pattern` against a value of type `ty`, declaring the values it
    /// binds in the current scope. Returns what it compiles to, or `None` if
    /// it has an error or `ty` is unknown, along with which values it matches.
    pub(super) fn pattern(
        &mut self,
        pattern: &'a Pattern,
        ty: Option<&Type>,
    ) -> (Option<ir::Pattern>, Space) {
        let Some(ty) = ty else {
            // The values it binds are still declared, so their uses aren't
            // reported as undeclared.
            for (binding, span) in bindings(pattern) {
                self.declare_local(binding, span, Mutability::Let, None);
            }
            return (None, Space::Any);
        };
        let mismatch = |checker: &mut Self| {
            checker.error(
                pattern.span,
                CheckError::PatternMismatch {
                    pattern: pattern.to_string(),
                    ty: ty.clone(),
                },
            );
            for (binding, span) in bindings(pattern) {
                checker.declare_local(binding, span, Mutability::Let, None);
            }
            (None, Space::Any)
        };
        match &pattern.kind {
            PatternKind::Any => (Some(ir::Pattern::Any), Space::Any),
            PatternKind::Binding(name) => {
                let local =
                    self.declare_local(name, pattern.span, Mutability::Let, Some(ty.clone()));
                (Some(ir::Pattern::Binding(local)), Space::Any)
            }
            PatternKind::Int { value, negative } => {
                let Some((bits, signed)) = self.int_type(ty) else {
                    return mismatch(self);
                };
                let value = if *negative {
                    -i128::from(*value)
                } else {
                    i128::from(*value)
                };
                let (min, max) = int_range(bits, signed);
                if !(min..=max).contains(&value) {
                    self.error(
                        pattern.span,
                        CheckError::IntOutOfRange {
                            value,
                            ty: ty.clone(),
                            min,
                            max,
                        },
                    );
                    return (None, Space::Any);
                }
                // Stored as its bits, like an integer literal.
                let bits = (value as u128 & (u128::MAX >> (128 - bits))) as u64;
                (
                    Some(ir::Pattern::Int(bits)),
                    Space::Ctor(Ctor::Int(value), vec![]),
                )
            }
            PatternKind::Bool(value) if *ty == self.std_type("Bool") => (
                Some(ir::Pattern::Bool(*value)),
                Space::Ctor(Ctor::Bool(*value), vec![]),
            ),
            PatternKind::Bool(_) => mismatch(self),
            PatternKind::Tuple(elements) => {
                // The labels must be the ones the tuple is built with.
                let types = self.elements(ty.id).map(<[_]>::to_vec).filter(|types| {
                    types.len() == elements.len()
                        && types
                            .iter()
                            .zip(elements)
                            .all(|(ty, element)| ty.label == element.label)
                });
                let Some(types) = types else {
                    return mismatch(self);
                };
                let mut lowered = vec![];
                let mut spaces = vec![];
                for (element, element_ty) in elements.iter().zip(&types) {
                    let (pattern, space) = self.pattern(&element.pattern, Some(&element_ty.ty));
                    lowered.push(pattern);
                    spaces.push(space);
                }
                let lowered = lowered.into_iter().collect::<Option<_>>();
                (
                    lowered.map(ir::Pattern::Tuple),
                    Space::Ctor(Ctor::Tuple, spaces),
                )
            }
            PatternKind::Option {
                ty: type_name,
                option,
                value,
            } => {
                let found = self.option_pattern(ty, type_name, option, pattern.span);
                let value_ty = found.as_ref().and_then(|(_, value_ty)| value_ty.as_ref());
                let (value, space) = match value {
                    Some(value) => self.pattern(value, value_ty),
                    None => (Some(ir::Pattern::Any), Space::Any),
                };
                let Some((index, _)) = found else {
                    return (None, Space::Any);
                };
                (
                    value.map(|value| ir::Pattern::Option {
                        index,
                        value: Box::new(value),
                    }),
                    Space::Ctor(Ctor::Option(index), vec![space]),
                )
            }
        }
    }
    /// Finds the option called `option` of `type_name`, which must be `ty`,
    /// returning its index and the type of its value, as `Checker::option`
    /// does, or `None` if it has an error.
    pub(super) fn option_pattern(
        &mut self,
        ty: &Type,
        type_name: &TypeName,
        option: &str,
        span: Span,
    ) -> Option<(usize, Option<Type>)> {
        // A generic type's arguments can be left out, since they're the
        // matched value's.
        let found = self.type_in_context(type_name, &[Some(ty)])?;
        if found != *ty {
            self.error(
                span,
                CheckError::WrongPatternType {
                    expected: ty.clone(),
                    found,
                },
            );
            return None;
        }
        let Some(option) = self.option(&found, option) else {
            let option = option.to_string();
            self.error(span, CheckError::UnknownOption { ty: found, option });
            return None;
        };
        Some(option)
    }
    /// Reports the patterns of `arms`, which match `spaces`, that can't be
    /// reached, since the ones before them match everything they do, and the
    /// values of type `ty` that none of them match, for the `match` at `span`.
    pub(super) fn check_exhaustive(
        &mut self,
        ty: &Type,
        span: Span,
        arms: &[MatchArm],
        spaces: Vec<Space>,
    ) {
        // A few missing values are shown, and one more shows there are more.
        const SHOWN: usize = 3;

        let mut rows: Vec<Vec<Space>> = vec![];
        let mut errors = vec![];
        let tys = std::slice::from_ref(ty);
        for (arm, space) in arms.iter().zip(spaces) {
            if self
                .unmatched(&rows, std::slice::from_ref(&space), tys, 1)
                .is_empty()
            {
                errors.push((
                    arm.pattern.span,
                    CheckError::UnreachablePattern(arm.pattern.to_string()),
                ));
            }
            rows.push(vec![space]);
        }
        let missing = self.unmatched(&rows, &[Space::Any], tys, SHOWN + 1);
        if !missing.is_empty() {
            let shown = missing.iter().take(SHOWN);
            errors.push((
                span,
                CheckError::MissingPatterns {
                    patterns: shown
                        .map(|values| self.show_space(&values[0], ty))
                        .collect(),
                    more: missing.len() > SHOWN,
                },
            ));
        }
        for (span, error) in errors {
            self.error(span, error);
        }
    }
    /// Up to `limit` values that `values` matches but none of `rows` do,
    /// where `values` and each row match a value of each of `tys` in turn.
    ///
    /// This is the usual way of checking patterns: values are split up by
    /// how they're made, as far as patterns can tell, such as which option
    /// of a choice they are, and each way is checked against the rows that
    /// match it.
    pub(super) fn unmatched(
        &self,
        rows: &[Vec<Space>],
        values: &[Space],
        tys: &[Type],
        limit: usize,
    ) -> Vec<Vec<Space>> {
        let Some((first, rest)) = values.split_first() else {
            // Nothing is left to match, so the values are unmatched only if
            // there are no rows.
            return if rows.is_empty() {
                vec![vec![]]
            } else {
                vec![]
            };
        };
        let (ty, rest_tys) = (&tys[0], &tys[1..]);
        if let Space::Ctor(ctor, parts) = first {
            return self.unmatched_ctor(rows, ctor, parts.clone(), rest, ty, rest_tys, limit);
        }

        let ctors = self.ctors(ty);
        // When no row is left, every value is unmatched, which is shown as
        // `_` rather than each way of making it, unless there are no values,
        // as for `Never`.
        if rows.is_empty() && ctors.as_ref().is_none_or(|ctors| !ctors.is_empty()) {
            return vec![vec![Space::Any; values.len()]];
        }

        let mut used: Vec<&Ctor> = vec![];
        for row in rows {
            if let Space::Ctor(ctor, _) = &row[0]
                && !used.contains(&ctor)
            {
                used.push(ctor);
            }
        }
        // Each way of making a value that a row uses is checked against the
        // rows that match it, in the order they're declared if they're known.
        let checked: Vec<&Ctor> = match &ctors {
            Some(ctors) => ctors.iter().filter(|ctor| used.contains(ctor)).collect(),
            None => used.clone(),
        };
        let mut found = vec![];
        for ctor in checked {
            let parts = vec![Space::Any; self.ctor_types(ty, ctor).len()];
            let left = limit - found.len();
            found.extend(self.unmatched_ctor(rows, ctor, parts, rest, ty, rest_tys, left));
            if found.len() >= limit {
                return found;
            }
        }

        // A value made any other way is only matched by the rows that match
        // anything first. It's shown as each way that no row uses, if they're
        // known, and `_` otherwise.
        let unused: Vec<Space> = match &ctors {
            Some(ctors) => ctors
                .iter()
                .filter(|ctor| !used.contains(ctor))
                .map(|ctor| {
                    let parts = vec![Space::Any; self.ctor_types(ty, ctor).len()];
                    Space::Ctor(ctor.clone(), parts)
                })
                .collect(),
            None => vec![Space::Any],
        };
        if unused.is_empty() {
            return found;
        }
        let defaults: Vec<_> = rows
            .iter()
            .filter(|row| matches!(row[0], Space::Any))
            .map(|row| row[1..].to_vec())
            .collect();
        let others = self.unmatched(&defaults, rest, rest_tys, limit - found.len());
        for first in unused {
            for other in &others {
                found.push(
                    iter::once(first.clone())
                        .chain(other.iter().cloned())
                        .collect(),
                );
                if found.len() >= limit {
                    return found;
                }
            }
        }
        found
    }
    /// Like `unmatched`, for values made with `ctor` from `parts`, followed
    /// by `rest`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn unmatched_ctor(
        &self,
        rows: &[Vec<Space>],
        ctor: &Ctor,
        parts: Vec<Space>,
        rest: &[Space],
        ty: &Type,
        rest_tys: &[Type],
        limit: usize,
    ) -> Vec<Vec<Space>> {
        let part_tys = self.ctor_types(ty, ctor);
        let arity = part_tys.len();
        // The rows that match values made this way, with their parts.
        let specialised: Vec<Vec<Space>> = rows
            .iter()
            .filter_map(|row| {
                let parts = match &row[0] {
                    Space::Ctor(found, parts) if found == ctor => parts.clone(),
                    Space::Ctor(..) => return None,
                    Space::Any => vec![Space::Any; arity],
                };
                Some(parts.into_iter().chain(row[1..].iter().cloned()).collect())
            })
            .collect();
        let values: Vec<_> = parts.into_iter().chain(rest.iter().cloned()).collect();
        let tys: Vec<_> = part_tys
            .into_iter()
            .chain(rest_tys.iter().cloned())
            .collect();
        self.unmatched(&specialised, &values, &tys, limit)
            .into_iter()
            .map(|mut found| {
                let rest = found.split_off(arity);
                iter::once(Space::Ctor(ctor.clone(), found))
                    .chain(rest)
                    .collect()
            })
            .collect()
    }
    /// Every way of making a value of `ty` that patterns can tell apart, or
    /// `None` if there are too many to list, such as for integers, or
    /// patterns can't tell them apart, such as for groups.
    pub(super) fn ctors(&self, ty: &Type) -> Option<Vec<Ctor>> {
        if *ty == self.std_type("Bool") {
            Some(vec![Ctor::Bool(false), Ctor::Bool(true)])
        } else if self.elements(ty.id).is_some() {
            Some(vec![Ctor::Tuple])
        } else {
            let options = self.options(ty.id)?;
            Some((0..options.len()).map(Ctor::Option).collect())
        }
    }
    /// The types of the parts of a value of `ty` made with `ctor`.
    pub(super) fn ctor_types(&self, ty: &Type, ctor: &Ctor) -> Vec<Type> {
        match ctor {
            Ctor::Int(_) | Ctor::Bool(_) => vec![],
            Ctor::Tuple => self
                .elements(ty.id)
                .unwrap_or_default()
                .iter()
                .map(|element| element.ty.clone())
                .collect(),
            // An option whose type is unknown has already been reported.
            Ctor::Option(index) => vec![
                self.options(ty.id).unwrap_or_default()[*index]
                    .ty
                    .clone()
                    .unwrap_or_else(|| self.std_type("Void")),
            ],
        }
    }
    /// How values of `ty` matching `space` are written as a pattern.
    pub(super) fn show_space(&self, space: &Space, ty: &Type) -> String {
        let Space::Ctor(ctor, parts) = space else {
            return "_".to_string();
        };
        let part_tys = self.ctor_types(ty, ctor);
        match ctor {
            Ctor::Int(value) => value.to_string(),
            Ctor::Bool(value) => value.to_string(),
            Ctor::Tuple => {
                let elements: Vec<_> = self
                    .elements(ty.id)
                    .unwrap_or_default()
                    .iter()
                    .zip(parts.iter().zip(&part_tys))
                    .map(|(element, (part, part_ty))| {
                        let part = self.show_space(part, part_ty);
                        match &element.label {
                            Some(label) => format!("{label}: {part}"),
                            None => part,
                        }
                    })
                    .collect();
                format!("({})", elements.join(", "))
            }
            Ctor::Option(index) => {
                let option = self.options(ty.id).unwrap_or_default()[*index].name;
                match &parts[0] {
                    Space::Any => format!("{ty}.{option}"),
                    value => format!("{ty}.{option}({})", self.show_space(value, &part_tys[0])),
                }
            }
        }
    }
}
