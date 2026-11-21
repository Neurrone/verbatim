//! The matcher: a test on one node's properties, the node subset of the
//! extension matcher grammar (`books/extensions/src/reference/matchers.md`),
//! as one type with one evaluator, used by a batch's `Test`, its
//! `Descendants` and their pruning, and Core's plain reader.
//!
//! The evaluator reads lazily, through a callback, in the matcher's
//! order, and stops at the first value that decides: `all` at its first
//! false, `any` at its first true. A property is read at most once per
//! evaluation. A property the object has no value for reads as absent,
//! as the reader's own record of absent properties gives it, so the
//! absent form and a value form agree on one object whether it was read
//! lazily or in a remote program.

use serde::{Deserialize, Serialize};

use crate::State;
use crate::property::{PropertyId, PropertyValue};

/// A test on one node's properties.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Matcher {
    /// The property has this value.
    Is(PropertyId, PropertyValue),
    /// The object has no value for the property.
    Absent(PropertyId),
    /// The property has one of these values.
    IsAnyOf(PropertyId, Vec<PropertyValue>),
    /// The object's states hold this state.
    HasState(State),
    /// Every matcher holds; an empty list holds.
    All(Vec<Matcher>),
    /// Some matcher holds; an empty list does not.
    Any(Vec<Matcher>),
    /// The matcher does not hold.
    Not(Box<Matcher>),
}

impl Matcher {
    /// Every property the matcher can read, in the order it reads them,
    /// each once.
    #[must_use]
    pub fn properties(&self) -> Vec<PropertyId> {
        let mut properties = Vec::new();
        self.each(&mut |matcher| {
            if let Some(id) = matcher.property()
                && !properties.contains(&id)
            {
                properties.push(id);
            }
        });
        properties
    }

    /// Every value the matcher compares with, each with its property.
    #[must_use]
    pub fn values(&self) -> Vec<(PropertyId, &PropertyValue)> {
        let mut values = Vec::new();
        self.each(&mut |matcher| match matcher {
            Self::Is(id, value) => values.push((*id, value)),
            Self::IsAnyOf(id, list) => values.extend(list.iter().map(|value| (*id, value))),
            Self::Absent(_) | Self::HasState(_) | Self::All(_) | Self::Any(_) | Self::Not(_) => {}
        });
        values
    }

    /// Whether the matcher holds of the object whose properties `read`
    /// reads: `Ok(None)` for a property the object has no value for.
    ///
    /// # Errors
    ///
    /// The first error `read` gives; nothing after it is read.
    pub fn evaluate<E>(
        &self,
        read: &mut dyn FnMut(PropertyId) -> Result<Option<PropertyValue>, E>,
    ) -> Result<bool, E> {
        self.holds(&mut Read {
            read,
            known: Vec::new(),
        })
    }

    fn holds<E>(&self, values: &mut Read<'_, E>) -> Result<bool, E> {
        Ok(match self {
            Self::Is(id, value) => values.value(*id)?.as_ref() == Some(value),
            Self::Absent(id) => values.value(*id)?.is_none(),
            Self::IsAnyOf(id, list) => values
                .value(*id)?
                .as_ref()
                .is_some_and(|found| list.contains(found)),
            Self::HasState(state) => matches!(
                values.value(PropertyId::States)?,
                Some(PropertyValue::States(states)) if states.contains(*state)
            ),
            Self::All(list) => {
                for matcher in list {
                    if !matcher.holds(values)? {
                        return Ok(false);
                    }
                }
                true
            }
            Self::Any(list) => {
                for matcher in list {
                    if matcher.holds(values)? {
                        return Ok(true);
                    }
                }
                false
            }
            Self::Not(matcher) => !matcher.holds(values)?,
        })
    }

    /// The property this form reads itself, `None` for a combining form.
    const fn property(&self) -> Option<PropertyId> {
        match self {
            Self::Is(id, _) | Self::Absent(id) | Self::IsAnyOf(id, _) => Some(*id),
            Self::HasState(_) => Some(PropertyId::States),
            Self::All(_) | Self::Any(_) | Self::Not(_) => None,
        }
    }

    /// Calls `visit` on this matcher and every matcher inside it, in the
    /// order they are evaluated.
    fn each<'a>(&'a self, visit: &mut dyn FnMut(&'a Self)) {
        visit(self);
        match self {
            Self::All(list) | Self::Any(list) => {
                for matcher in list {
                    matcher.each(visit);
                }
            }
            Self::Not(matcher) => matcher.each(visit),
            Self::Is(..) | Self::Absent(_) | Self::IsAnyOf(..) | Self::HasState(_) => {}
        }
    }
}

/// One evaluation's reads: the callback, and each value it gave.
struct Read<'a, E> {
    read: &'a mut dyn FnMut(PropertyId) -> Result<Option<PropertyValue>, E>,
    known: Vec<(PropertyId, Option<PropertyValue>)>,
}

impl<E> Read<'_, E> {
    /// The value of `id`, read on its first use.
    fn value(&mut self, id: PropertyId) -> Result<&Option<PropertyValue>, E> {
        let place = if let Some(place) = self.known.iter().position(|(known, _)| *known == id) {
            place
        } else {
            let value = (self.read)(id)?;
            self.known.push((id, value));
            self.known.len().saturating_sub(1)
        };
        Ok(self.known.get(place).map_or(&None, |(_, value)| value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Role, StateSet};

    /// An object's values, and the properties read of it, in order.
    struct Object {
        values: Vec<(PropertyId, PropertyValue)>,
        reads: Vec<PropertyId>,
    }

    impl Object {
        fn new(values: Vec<(PropertyId, PropertyValue)>) -> Self {
            Self {
                values,
                reads: Vec::new(),
            }
        }

        fn matches(&mut self, matcher: &Matcher) -> bool {
            let values = &self.values;
            let reads = &mut self.reads;
            matcher
                .evaluate::<()>(&mut |id| {
                    reads.push(id);
                    Ok(values
                        .iter()
                        .find(|(known, _)| *known == id)
                        .map(|(_, value)| value.clone()))
                })
                .unwrap()
        }
    }

    fn button() -> Object {
        Object::new(vec![
            (PropertyId::Role, PropertyValue::Role(Role::Button)),
            (PropertyId::Name, PropertyValue::Text("OK".into())),
            (
                PropertyId::States,
                PropertyValue::States(StateSet::new().with(State::Focused)),
            ),
        ])
    }

    fn name(text: &str) -> PropertyValue {
        PropertyValue::Text(text.into())
    }

    #[test]
    fn each_form_holds_as_its_name_says() {
        let cases = [
            (Matcher::Is(PropertyId::Name, name("OK")), true),
            (Matcher::Is(PropertyId::Name, name("Cancel")), false),
            (Matcher::Is(PropertyId::Value, name("OK")), false),
            (Matcher::Absent(PropertyId::Value), true),
            (Matcher::Absent(PropertyId::Name), false),
            (
                Matcher::IsAnyOf(PropertyId::Name, vec![name("Cancel"), name("OK")]),
                true,
            ),
            (
                Matcher::IsAnyOf(PropertyId::Name, vec![name("Cancel")]),
                false,
            ),
            (Matcher::IsAnyOf(PropertyId::Value, vec![name("OK")]), false),
            (Matcher::HasState(State::Focused), true),
            (Matcher::HasState(State::Checked), false),
            (Matcher::All(Vec::new()), true),
            (Matcher::Any(Vec::new()), false),
            (
                Matcher::Not(Box::new(Matcher::HasState(State::Checked))),
                true,
            ),
        ];
        for (matcher, holds) in cases {
            assert_eq!(button().matches(&matcher), holds, "{matcher:?}");
        }
    }

    #[test]
    fn a_state_test_of_an_object_with_no_states_does_not_hold() {
        let mut object = Object::new(Vec::new());
        assert!(!object.matches(&Matcher::HasState(State::Focused)));
    }

    #[test]
    fn all_stops_at_its_first_false_and_any_at_its_first_true() {
        let all = Matcher::All(vec![
            Matcher::Is(PropertyId::Role, PropertyValue::Role(Role::Dialog)),
            Matcher::Is(PropertyId::Name, name("OK")),
        ]);
        let mut object = button();
        assert!(!object.matches(&all));
        assert_eq!(object.reads, [PropertyId::Role]);

        let any = Matcher::Any(vec![
            Matcher::Is(PropertyId::Role, PropertyValue::Role(Role::Button)),
            Matcher::Is(PropertyId::Name, name("OK")),
        ]);
        let mut object = button();
        assert!(object.matches(&any));
        assert_eq!(object.reads, [PropertyId::Role]);
    }

    #[test]
    fn reads_follow_the_matchers_order_and_take_each_property_once() {
        let matcher = Matcher::Any(vec![
            Matcher::Not(Box::new(Matcher::HasState(State::Focused))),
            Matcher::All(vec![
                Matcher::HasState(State::Focusable),
                Matcher::Is(PropertyId::Name, name("Cancel")),
            ]),
            Matcher::Absent(PropertyId::Value),
        ]);
        let mut object = Object::new(vec![(
            PropertyId::States,
            PropertyValue::States(StateSet::new().with(State::Focused).with(State::Focusable)),
        )]);
        assert!(object.matches(&matcher));
        assert_eq!(
            object.reads,
            [PropertyId::States, PropertyId::Name, PropertyId::Value]
        );
        assert_eq!(
            matcher.properties(),
            [PropertyId::States, PropertyId::Name, PropertyId::Value]
        );
    }

    #[test]
    fn a_failed_read_ends_the_evaluation_with_its_error() {
        let matcher = Matcher::All(vec![
            Matcher::Absent(PropertyId::Name),
            Matcher::Absent(PropertyId::Value),
        ]);
        let mut reads = 0;
        let result = matcher.evaluate(&mut |_| {
            reads += 1;
            Err("the call failed")
        });
        assert_eq!((result, reads), (Err("the call failed"), 1));
    }

    #[test]
    fn a_matcher_lists_the_values_it_compares_with() {
        let matcher = Matcher::All(vec![
            Matcher::Is(PropertyId::Name, name("OK")),
            Matcher::Not(Box::new(Matcher::IsAnyOf(
                PropertyId::Value,
                vec![name("1"), name("2")],
            ))),
        ]);
        assert_eq!(
            matcher.values(),
            [
                (PropertyId::Name, &name("OK")),
                (PropertyId::Value, &name("1")),
                (PropertyId::Value, &name("2")),
            ]
        );
    }
}
