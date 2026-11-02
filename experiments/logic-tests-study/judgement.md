# Mutation baseline for M4.6

- Date: 2026-11-02 to 2026-11-03
- Commit: be5bd5b9
- Tool: cargo-mutants 27.1.0, run in place in a worktree, one mutant at a time (`--jobs 1` is refused together with `--in-place`, and one job is the default), each crate's own tests only. `CARGO_TARGET_DIR` inside the worktree, `VERBATIM_WX_DIR` at the main checkout's `target/wxwidgets`.
- Run time: 55 minutes of wall clock for all six runs (reader-rules 3 min, outpost-logic 31 min, listener-logic 4 min, core 9 + 5 + 4 min), first build included.
- Command lines, run from the worktree root:
  - `cargo mutants --in-place -p verbatim-reader-rules -f crates/verbatim-reader-rules/src/in_flight.rs`
  - `cargo mutants --in-place -p verbatim-outpost-logic -f` each of `lib.rs`, `watch.rs`, `gap_record.rs`, `follow.rs`, `verdicts.rs`, `hold.rs`, `intake.rs` under `crates/verbatim-outpost-logic/src/`
  - `cargo mutants --in-place -p verbatim-listener-logic -f crates/verbatim-listener-logic/src/schedule.rs -f crates/verbatim-listener-logic/src/foreground.rs`
  - `cargo mutants --in-place -p verbatim-core -f crates/verbatim-core/src/queries.rs`
  - `cargo mutants --in-place -p verbatim-core -f crates/verbatim-core/src/reduce.rs -F 'foreground_changed|leaves_focus|observed_before_foreground_change|outpost_ended'`
  - `cargo mutants --in-place -p verbatim-core -f crates/verbatim-core/src/terminal_model.rs -F 'ask_model|model_asks|answered_read|reply|answerable'`

## verbatim-reader-rules

Caught 42, missed 4, unviable 53, timeout 0.

### in_flight.rs: InFlight<T>::deadline

- in_flight.rs:425: replace body with None
- in_flight.rs:425: replace body with Some(0)
- in_flight.rs:425: replace body with Some(1)

### in_flight.rs: InFlight<T>::item_mut

- in_flight.rs:430: replace body with None

## verbatim-outpost-logic

Caught 413, missed 101, unviable 101, timeout 0.

### follow.rs: FocusPath::of

- follow.rs:149: replace body with Default::default()
- follow.rs:157: replace != with ==

### follow.rs: FocusPath::is_cut_at

- follow.rs:167: replace body with false
- follow.rs:167: replace body with true
- follow.rs:168: replace && with ||
- follow.rs:168: replace == with !=
- follow.rs:169: replace && with ||

### follow.rs: FocusPath::route_to

- follow.rs:178: replace == with !=
- follow.rs:178: replace body with None
- follow.rs:183: replace + with *
- follow.rs:183: replace + with -

### follow.rs: FollowState::retarget

- follow.rs:485: replace == with !=
- follow.rs:501: replace += with *=
- follow.rs:501: replace += with -=

### follow.rs: FollowState::follows

- follow.rs:513: replace == with !=

### follow.rs: FollowState::release_groups

- follow.rs:556: replace += with *=
- follow.rs:556: replace += with -=

### follow.rs: apply

- follow.rs:648: replace == with !=
- follow.rs:661: replace && with ||
- follow.rs:661: replace == with !=
- follow.rs:661: replace == with !=

### follow.rs: apply_watch

- follow.rs:696: replace && with ||

### follow.rs: answer_to_judge

- follow.rs:732: replace == with !=
- follow.rs:734: delete field event from struct held::Focus expression
- follow.rs:735: delete field simple_child from struct held::Focus expression

### gap_record.rs: GapRecord::foreground

- gap_record.rs:345: replace && with ||
- gap_record.rs:345: replace match guard open.top_level == window && open.awaits == Awaits::Focus with true

### hold.rs: Hold::held

- hold.rs:91: replace body with ::std::iter::empty()

### hold.rs: forwarded_time

- hold.rs:172: replace match guard forwarded(fact) with true

### hold.rs: forwarded

- hold.rs:179: replace body with true

### intake.rs: (no function named)

- intake.rs:176: delete -

### intake.rs: Marks::drops_on_arrival

- intake.rs:392: delete match arm Item::Msaa{kind:WinKind::Caret, ..}

### intake.rs: Intake::push

- intake.rs:521: replace < with <=
- intake.rs:521: replace < with ==
- intake.rs:521: replace < with >
- intake.rs:523: replace -= with +=
- intake.rs:523: replace -= with /=

### intake.rs: Intake::next

- intake.rs:600: replace > with >=
- intake.rs:605: replace += with *=

### intake.rs: Intake::cancel

- intake.rs:649: delete !

### intake.rs: Intake::batch_number

- intake.rs:711: replace body with 0
- intake.rs:711: replace body with 1

### intake.rs: Intake::waiting_carets

- intake.rs:718: replace body with vec![]
- intake.rs:722: replace / with %
- intake.rs:722: replace / with *
- intake.rs:722: replace >= with <

### intake.rs: old_focus_events_first

- intake.rs:786: replace + with *

### intake.rs: remove_waiting

- intake.rs:873: delete !
- intake.rs:875: replace + with *
- intake.rs:875: replace + with -

### intake.rs: caret_after_update

- intake.rs:975: replace && with ||
- intake.rs:983: replace + with *
- intake.rs:988: replace && with ||

### intake.rs: admit

- intake.rs:1128: replace += with *=

### lib.rs: Reducer::input

- lib.rs:728: delete !

### lib.rs: Reducer::tick

- lib.rs:748: replace && with ||

### lib.rs: Reducer::gap_bound_tested

- lib.rs:834: replace && with ||
- lib.rs:834: replace match guard reads && self.intake.gap().is_open() with true

### lib.rs: Reducer::send

- lib.rs:945: replace == with !=

### lib.rs: Reducer::answer_watch

- lib.rs:1080: delete field awaited_at_us from struct EventTiming expression
- lib.rs:1081: delete field awaited_calls from struct EventTiming expression
- lib.rs:1082: delete field calls from struct EventTiming expression
- lib.rs:1083: delete field published_at_us from struct EventTiming expression

### lib.rs: Reducer::released

- lib.rs:1139: delete !

### lib.rs: Reducer::finished

- lib.rs:1276: replace && with ||
- lib.rs:1300: replace || with &&

### lib.rs: Reducer::pump

- lib.rs:1430: replace += with *=

### lib.rs: Reducer::intake_batch

- lib.rs:1468: replace body with 0
- lib.rs:1468: replace body with 1

### lib.rs: Reducer::run

- lib.rs:1484: delete field item from struct Entry expression
- lib.rs:1538: replace match guard now.wall_us >= walk_until_us with false
- lib.rs:1545: replace match guard now.wall_us >= walk_until_us with true

### lib.rs: Reducer::live

- lib.rs:1560: replace body with vec![]

### lib.rs: Reducer::dispatch_finish

- lib.rs:1665: replace == with !=

### lib.rs: Reducer::waiting

- lib.rs:1681: replace body with vec![]
- lib.rs:1682: delete match arm Work::Entry{entry:Entry{item:Item::Query{query:Query::Text{op:TextOp::AwaitCaret(watch), ..}, ..}, ..}, ..}

### lib.rs: waiting_caret

- lib.rs:1733: replace body with None
- lib.rs:1734: delete match arm Item::Uia(UiaEvent{kind:UiaKind::TextSelection, parts, ..})
- lib.rs:1739: delete match arm Item::Msaa{kind:WinKind::ConsoleCaret, hwnd, ..}
- lib.rs:1744: delete match arm Item::Msaa{kind:WinKind::Caret | WinKind::TextSelectionChange, hwnd, ..}

### lib.rs: clears_absorb

- lib.rs:1772: replace body with true

### lib.rs: owed_clears_absorb

- lib.rs:1790: replace body with false
- lib.rs:1790: replace body with true

### lib.rs: budget

- lib.rs:1806: delete match arm Query::FocusNow
- lib.rs:1807: delete match arm Query::Ancestors{..} | Query::DumpTree{..}
- lib.rs:1812: delete match arm Query::Text{..}

### lib.rs: watch_ended

- lib.rs:1850: delete field calls from struct EventTiming expression
- lib.rs:1851: delete field published_at_us from struct EventTiming expression

### lib.rs: gap_report

- lib.rs:1873: delete match arm OutpostToCore::Reply{outcome:QueryOutcome::Done(QueryResult::Focus(now)), ..}

### verdicts.rs: Probed::lasts

- verdicts.rs:42: replace body with false
- verdicts.rs:42: replace body with true

### verdicts.rs: Kept::judges

- verdicts.rs:98: replace < with >
- verdicts.rs:102: replace match guard at_us >= until with false

### verdicts.rs: Verdicts::lasts

- verdicts.rs:146: replace body with false
- verdicts.rs:148: replace && with ||
- verdicts.rs:148: replace == with !=

### verdicts.rs: Verdicts::apply

- verdicts.rs:171: replace == with !=
- verdicts.rs:171: replace match guard window.current.owner == kept.owner with false
- verdicts.rs:171: replace match guard window.current.owner == kept.owner with true
- verdicts.rs:172: replace >= with <

### watch.rs: Watch::take

- watch.rs:237: replace match guard open.node_id == node with true

## verbatim-listener-logic

Caught 105, missed 22, unviable 9, timeout 0.

### schedule.rs: Schedule<J>::stopping

- schedule.rs:422: replace body with false
- schedule.rs:422: replace body with true

### schedule.rs: Schedule<J>::focus_pending

- schedule.rs:465: replace == with !=

### schedule.rs: Schedule<J>::push

- schedule.rs:618: replace && with ||
- schedule.rs:633: replace != with ==
- schedule.rs:658: replace + with *

### schedule.rs: Schedule<J>::next

- schedule.rs:734: replace == with !=

### schedule.rs: Schedule<J>::returned

- schedule.rs:753: replace && with ||
- schedule.rs:775: replace < with <=
- schedule.rs:775: replace < with ==
- schedule.rs:775: replace < with >

### schedule.rs: Schedule<J>::abandon

- schedule.rs:883: replace && with ||

### schedule.rs: Schedule<J>::published

- schedule.rs:941: replace && with ||

### schedule.rs: Schedule<J>::drop_waiting

- schedule.rs:972: replace || with &&

### schedule.rs: Schedule<J>::foreground

- schedule.rs:1011: replace == with !=

### schedule.rs: Schedule<J>::outpost_ended

- schedule.rs:1023: replace body with ()
- schedule.rs:1026: delete !
- schedule.rs:1026: replace && with ||
- schedule.rs:1026: replace && with ||
- schedule.rs:1026: replace == with !=

### schedule.rs: Schedule<J>::spawn_failed

- schedule.rs:1035: replace body with ()
- schedule.rs:1038: delete !

## verbatim-core

Caught 76, missed 24, unviable 26, timeout 0.

### queries.rs: Queries::find_mut

- queries.rs:230: replace && with ||

### queries.rs: Queries::next

- queries.rs:302: replace body with 0
- queries.rs:302: replace body with 1

### queries.rs: Queries::set_traces

- queries.rs:310: replace == with !=
- queries.rs:310: replace body with ()

### queries.rs: Queries::set_trace

- queries.rs:319: replace body with ()
- queries.rs:320: delete match arm Effect::ForKey(key, inner)
- queries.rs:326: replace >= with <

### queries.rs: outpost_gone

- queries.rs:435: replace && with ||

### reduce.rs: observed_before_foreground_change

- reduce.rs:519: replace < with <=

### reduce.rs: leaves_focus

- reduce.rs:604: replace < with <=

### reduce.rs: outpost_ended

- reduce.rs:1418: replace == with !=
- reduce.rs:1425: replace == with !=
- reduce.rs:1430: delete match arm Then::Caret(PendingCaret{node, ..}) | Then::Text(PendingText{node, ..})
- reduce.rs:1431: replace == with !=
- reduce.rs:1438: replace == with !=

### terminal_model.rs: answered_read

- terminal_model.rs:905: replace body with None
- terminal_model.rs:905: replace body with Some((Default::default(), None, vec![]))
- terminal_model.rs:912: delete match arm TextReply::TerminalSnapshot(snapshot)
- terminal_model.rs:916: delete match arm TextReply::TerminalUnchanged

### terminal_model.rs: reply

- terminal_model.rs:973: delete !
- terminal_model.rs:974: delete !
- terminal_model.rs:974: replace && with ||

### terminal_model.rs: answerable

- terminal_model.rs:1098: delete match arm Effect::ForKey(_, effect)

