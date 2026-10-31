# Property-based and model-based (state-machine) testing in industry: case studies, costs, and failures

Terms used here. Property-based testing (PBT): a test states a property that should hold for all inputs, and a library generates random inputs to check it. Model-based or stateful testing: the generated input is a sequence of operations; each runs against both the real system and a simple reference model, and outputs and invariants are compared after each step. Shrinking: when a failing input is found, the library searches for a smaller input that still fails. Oracle: whatever decides whether an output is correct.

Source quality: Hughes 2016 was read in full (pdftotext of the PDF). Goldstein et al. 2022 (HATRA) was read in full. The ShardStore paper's full text could not be fetched (404, 503 and 403 from three hosts), so its details come from the abstract plus a secondary summary by Murat Demirbas, marked below. The ICSE 2024 Jane Street paper's full text could not be fetched (503); only the abstract-level findings are cited.

## Erlang QuickCheck and Quviq case studies (Volvo/AUTOSAR, Klarna dets, Ericsson, Basho Riak, Dropbox) and Hughes's lessons on writing properties

### Takeaway
The Quviq record is strong and quantified: a stateful model of under 100 lines found the Klarna dets corruption bugs that six weeks of manual hunting had not, and 20,000 lines of models found more than 200 problems in a million lines of AUTOSAR C. Hughes's own lessons point the same way each time: many failures are bugs in the model, shrinking is where most of the value lies, and writing the specification is the real weakness of the method.

### Cited Findings
- Volvo/AUTOSAR, the largest QuickCheck project to that date: the team read about 3,000 pages of standards, formalized them in 20,000 lines of QuickCheck code, and tested a million lines of C code from 6 suppliers. They found more than 200 problems, "well over 100" of which were ambiguities or inconsistencies in the standard itself. Where comparison was possible, the QuickCheck code was "an order of magnitude smaller" than traditional TTCN3 suites "but tests more". — [Hughes, Experiences with QuickCheck, 2016 (PDF)](https://www.cs.tufts.edu/~nr/cs257/archive/john-hughes/quviq-testing.pdf)
- Volvo, the extra machinery the project needed (a hidden cost): property-based mocking; ways to cluster component models into a subsystem model, because AUTOSAR is specified as components but implemented as one block; tooling to answer "does your model cover this test case?"; and requirements-coverage reporting, which led the authors "to question what testing a requirement even means". Three of six "official" AUTOSAR CAN test cases turned out not to be covered by the model, because they were inconsistent with the standard. — [Hughes 2016](https://www.cs.tufts.edu/~nr/cs257/archive/john-hughes/quviq-testing.pdf)
- Volvo, a shrinking pitfall: "QuickCheck's shrinking tended to isolate the same bug in each run," which is a poor fit for reporting on a component with several bugs. Quviq built methods to steer testing away from known problems toward areas where unknown bugs might lurk. — [Hughes 2016](https://www.cs.tufts.edu/~nr/cs257/archive/john-hughes/quviq-testing.pdf)
- Klarna dets: Klarna saw "bad object" and "premature eof" every other month, later once a week. Ericsson's dets maintainer had spent six weeks at Klarna hunting the bug, and the best theory involved files around 1 GB and rehashing. Hughes's dets model was "less than 100 lines", against an implementation of over 6,000 lines in four modules. He first ran tens of thousands of sequential tests to make the model consistent with the code; parallel testing then reused the same model with a few lines added. It found several race bugs, the corrupting ones after about 10 minutes of testing each. Each bug was provoked with at most one record and 5 to 6 API calls, and each was fixed within a day. — [Hughes 2016](https://www.cs.tufts.edu/~nr/cs257/archive/john-hughes/quviq-testing.pdf)
- One Klarna minimal case needed an open-close-open sequence that "was initially hard to believe" was necessary, plus three parallel operations: the first makes the server busy so the next two queue up. In other words, the state that triggers the bug is reachable only by a specific prefix. — [Hughes 2016](https://www.cs.tufts.edu/~nr/cs257/archive/john-hughes/quviq-testing.pdf)
- The race-detection method, PULSE plus QuickCheck parallel testing: the property is that parallel results must be explainable by some sequential interleaving of the same model. It is described in the ICFP 2009 paper. — [Claessen et al., Finding race conditions in Erlang with QuickCheck and PULSE](https://research.chalmers.se/en/publication/253486)
- Lessons Hughes draws from his queue example: "The same property can find many different bugs"; "Errors are often in the model, rather than the code. Calling something a specification does not make it right!"; and "Minimal failing test cases make debugging easy… Debugging a test that is 90% irrelevant is a nightmare." — [Hughes 2016](https://www.cs.tufts.edu/~nr/cs257/archive/john-hughes/quviq-testing.pdf)
- The general lesson: "formulating specifications is hard—and many developers struggle with it… The need for a specification is the real weakness of property-based testing." Bugs are found by "comparing two independent descriptions of the desired behaviour." — [Hughes 2016](https://www.cs.tufts.edu/~nr/cs257/archive/john-hughes/quviq-testing.pdf)
- Riak (Basho): QuickCheck found an eventual-consistency bug "present, not only in Riak, but in the original Amazon [Dynamo] paper". Dropbox's file synchronization was also tested "with some surprising results". Both used the state-machine framework "but added additional properties to express subtle correctness conditions". — [Hughes 2016](https://www.cs.tufts.edu/~nr/cs257/archive/john-hughes/quviq-testing.pdf). Riak slides say the first failing case was two failures in a short interval that caused data loss. — [Hughes, The Properties of Riak, 2012 slides](https://www.erlang-factory.com/upload/presentations/593/ThePropertiesofRiak.pdf) (search-snippet level only)
- Ericsson Media Proxy: Hughes's tutorial slides say generators were "lots of work", with messages of 1 to 2 pages of parameters each, but that state-machine models fit the problem well. Property-driven testing is often cheaper than ordinary testing, but "lots of work to develop specification" comes first. — [Hughes FMCAD 2011 tutorial slides](https://www.cs.utexas.edu/~hunt/fmcad/FMCAD11/slides/tutorial-a.pdf) (secondary: seen through a search summary only)
- "How to Specify It!" (TFP 2019, published 2020): Hughes injected eight buggy variants into a binary search tree and measured the bugs each property style caught. Invariant properties (4 written) caught 38%; postconditions (5) 79%; metamorphic properties (16) 90%; model-based properties (5) 100%. Model-based properties "may offer the best return on investment", paired with validity properties so that invalid generated data does not produce confusing failures. A single property can pass a wrong implementation. Computing expected outputs can be as hard as the function itself, and developers may copy the same bugs into the oracle. — [InfoQ summary of the paper and the Lambda Days 2020 talk (secondary)](https://www.infoq.com/news/2020/02/property-based-testing-guide); [paper record](https://research.chalmers.se/en/publication/517894)

### Inferences
- For Verbatim's reducers, the result that matters most is the 38% figure: invariant-only checks after each step were the weakest style in Hughes's experiment. Generated state-machine tests that only check invariants, with no comparison to a reference model and no postconditions, are likely to miss most behavioral bugs.
- Expect early failures to be model bugs. Hughes's practice of validating the model with many sequential runs before adding concurrency, or harder properties, is a cheap way to separate model bugs from real ones.
- Bugs like Klarna's live behind specific short prefixes, such as open-close-open. A generator must be able to produce lifecycle operations (open, close, reset, re-dispatch) often enough, or those states are never reached.

### Gaps
- No published effort figure in person-months for Volvo or Riak was found. Only the lines of code and the page counts above are available.
- The per-bug detail of the Dropbox study (Hughes, Pierce, Arts, Norell, ICST 2016, "Mysteries of Dropbox") was not fetched.

## Amazon S3 ShardStore (SOSP 2021)

### Takeaway
Executable reference models written in Rust, checked by property-based tests over operation sequences, plus crash-consistency properties and Loom and Shuttle for concurrency, kept 16 issues out of production. Engineers who were not formal-methods experts maintained the models. The main stated limits are that standard PBT cannot explore concurrency, Loom does not scale beyond small harnesses, and concurrent crashing executions were left unchecked.

### Cited Findings
- ShardStore is a new S3 key-value storage node. Correctness was split into independent properties, each checked with the best-suited tool. Executable reference models served as specifications. The checks "prevented 16 issues from reaching production, including subtle crash consistency and concurrency problems", and were "extended by non-formal-methods experts" as the system evolved. — [Bornholt et al., SOSP 2021, abstract](https://www.amazon.science/publications/using-lightweight-formal-methods-to-validate-a-key-value-storage-node-in-amazon-s3)
- The following details are secondary, from Murat Demirbas's summary of the paper ([blog, Oct 2021](https://muratbuffalo.blogspot.com/2021/10/using-lightweight-formal-methods-to.html)):
  - ShardStore is over 40K lines of Rust.
  - There is one reference model per component, with the same interface but a simpler implementation; for example, ReferenceIndex is a hash table where the real Index is an LSM tree. The models double as mocks in unit tests, so engineers keep them current.
  - The validation is split three ways: sequential crash-free runs against the model by PBT; sequential crashing runs, against a model refined to say what may be lost after a crash; and concurrent crash-free runs, checked by model checking.
  - PBT draws operation sequences from a team-defined alphabet. Arguments were biased toward corner cases, and failures were injected, to improve state coverage.
  - The crash properties are persistence (what a dependency reports as persisted must be readable after recovery) and forward progress (after a clean shutdown, every write reports persistent).
  - One bug needed three coincidences at once: a random UUID colliding with magic bytes, a chunk just large enough to spill onto a second page, and a crash that lost only that page.
  - Loom explores interleavings exhaustively for small critical code such as sharded reader-writer locks. Shuttle explores randomly for large end-to-end harnesses that Loom cannot scale to.
  - Limits: concurrent crashing executions are future work; standard PBT is unsuitable for concurrency; Loom does not scale.
  - What did not work: the team first planned models in Alloy, SPIN and Python and imagined tools to check the Rust against them. Discussing long-term maintenance led them to write the models in Rust instead, with the stated goal of lowering the marginal cost of validating future changes.

### Inferences
- The decisions that carried over directly: models in the implementation language, reused as mocks so they cannot fall out of date, and argument bias plus fault injection, because uniform random sequences do not reach the corner states. Verbatim's reducers are pure, so the Shuttle and Loom layer applies only to the hand-offs around them.
- The three-coincidence bug shows the value of biased generation. Uniform UUIDs would almost never collide with magic bytes; the generator had to be built to make such collisions likely.

### Gaps
- The full text could not be retrieved, so these remain unverified: the paper's per-bug table (which technique found which of the 16 issues), model sizes relative to code, any shrinking or minimization problems, run times, and the engineers' quotes. My recollection is that the paper reports model size as a small fraction of the code and discusses custom minimization, but I could not confirm this, so it is not claimed.

## Other industrial reports and studies of PBT in practice (Jane Street, Hypothesis users, Rust proptest)

### Takeaway
The interview studies (N=7 Hypothesis users in 2022; N=30 Jane Street developers in 2024) find PBT valued for complex components where properties are easy to state. They report the same weaknesses each time: imagining properties, writing generators that hit the right distribution, slow tests, randomness in CI, and no good way to judge whether the tests are effective.

### Cited Findings
- ICSE 2024, 30 interviews at Jane Street: PBT is used mainly on components of complex systems, in "high-leverage" settings where properties are easy to identify. Its main weaknesses are "the relative complexity of writing properties and random data generators and… the difficulty of evaluating their effectiveness". The opportunities named are performance, differential testing, input generation, test-case reduction, and measuring test effectiveness. — [Goldstein et al., Property-Based Testing in Practice, ICSE 2024](https://cis.upenn.edu/~bcpierce/papers/icse24-pbt-in-practice); [DOI](https://doi.org/10.1145/3597503.3639581)
- HATRA 2022, 7 Hypothesis users. Positive: one participant "found probably half a dozen major corner-case bugs"; another called PBT "1000 times better than the alternative". — [Goldstein et al., Some Problems with Properties, 2022](https://harrisongoldste.in/papers/hatra2022.pdf)
- HATRA 2022, specification failures: participants either under-specified (testing only that a program does not crash) or over-specified (comparing to a behaviorally complete model that had to be implemented separately), which led to tests that were weak, or brittle and time-consuming. One participant described "formulating the right property in the first place" as the hard part. Another, testing financial risk computations, could not find the properties. — [HATRA 2022](https://harrisongoldste.in/papers/hatra2022.pdf)
- HATRA 2022, testability cost: one participant needed "compose points where you can integrate [PBT] with the system". The cost of restructuring code often outstripped the benefit, so they did not use PBT. — [HATRA 2022](https://harrisongoldste.in/papers/hatra2022.pdf)
- HATRA 2022, generators: rejection sampling of valid network ACLs "was too slow to be useful", and a by-construction generator "may or may not be worth it". Default generators produced poor distributions: tests "running for quite a long time" were "missing some useful use cases". — [HATRA 2022](https://harrisongoldste.in/papers/hatra2022.pdf)
- HATRA 2022, workflow: properties were run only in CI because they were too slow locally, and random discovery made CI "a bit like Russian roulette, because sometimes the code breaks while you're working on something totally unrelated". Another participant kept properties out of CI for that reason. — [HATRA 2022](https://harrisongoldste.in/papers/hatra2022.pdf)
- HATRA 2022, adoption: managers were excited by a severe bug found, but would not invest further: "can they find somebody to write those tests and maintain those tests?" A participant feared that explaining Hypothesis to newcomers added one more complicated thing. — [HATRA 2022](https://harrisongoldste.in/papers/hatra2022.pdf)
- Hypothesis stateful shrinking: a 2019 issue reports that rule-based stateful shrinking did not reach a minimal example for a Towers of Hanoi machine. The proposed fix was to try deleting pairs of adjacent rule calls. — [Hypothesis issue #1857](https://github.com/HypothesisWorks/hypothesis/issues/1857)
- Rust proptest-state-machine: preconditions are checked during generation and during shrinking, so shrinking keeps sequences valid. The docs warn that preconditions must cover the state-dependent invariants shrinking can break, and that they rely on global rejection filtering, with its usual costs. The reference model is generated before transitions run against the system under test. — [proptest-state-machine docs](https://docs.rs/proptest-state-machine/0.8.0/proptest_state_machine/strategy/trait.ReferenceStateMachine.html); [author's blog post](https://tzemanovic.codeberg.page/posts/state-machine-testing-with-proptest/)
- The sibling crate proptest-stateful shrinks only by removing operations, not by simplifying them: shrinking individual operations "tends to break preconditions in a way that is difficult to compensate for". — [proptest-stateful docs](https://docs.rs/proptest-stateful/latest/proptest_stateful/)

### Inferences
- For a pure reducer, the "compose points" problem is mostly solved already: the reducer is the boundary. This is the setting where PBT reports are most positive.
- To avoid the "Russian roulette" problem, run generated tests in CI with a fixed seed and case count, so each run is reproducible and meets the repository's one-code-path rule. Run deeper randomized searches as a separate, deliberate job, and add every failure found to the fixed corpus as a regression case.
- Prefer generators that build valid operations from the current model state over generators that filter out invalid ones, because rejection was both slow and a source of distribution bias in the reports.

### Gaps
- No primary material was found from Datadog, Stripe or Mozilla on stateful PBT in this pass, and none from Dropbox beyond Hughes's mention.
- No quantitative study was found on how often PBT finds bugs relative to example tests in industry. The ICSE 2024 paper is qualitative. A 2025 ESEM paper on PBT in Python projects appeared in search results but was not read.
- No David MacIver primary essay on failure modes was retrieved. Search results attributing quotes to him came from unverifiable skill-marketplace pages and are excluded.

## Reported failure modes

### Takeaway
The recurring failures are these: specifications too weak (crash-only or invariant-only) or too strong (a full re-implementation); errors in the model; generators that miss rare states or reject too much; shrinking that finds the same bug each time or stalls on stateful sequences; slowness that pushes tests into CI where randomness causes flakiness; and organizations unwilling to maintain the tests.

### Cited Findings
- Weak properties: invariant properties caught 38% of injected bugs, against 100% for model-based ones. One property alone can accept a wrong implementation. — [InfoQ on How to Specify It!](https://www.infoq.com/news/2020/02/property-based-testing-guide)
- Properties that restate the implementation: computing the oracle can be as hard as the function, and the same bugs get copied in. — [InfoQ on How to Specify It!](https://www.infoq.com/news/2020/02/property-based-testing-guide); over-specified models were found brittle and time-consuming. — [HATRA 2022](https://harrisongoldste.in/papers/hatra2022.pdf)
- Model errors: "Errors are often in the model, rather than the code." — [Hughes 2016](https://www.cs.tufts.edu/~nr/cs257/archive/john-hughes/quviq-testing.pdf)
- Shrinking converging on one bug: [Hughes 2016](https://www.cs.tufts.edu/~nr/cs257/archive/john-hughes/quviq-testing.pdf). Stateful shrinking that is not minimal: [Hypothesis #1857](https://github.com/HypothesisWorks/hypothesis/issues/1857). Operations that cannot be shrunk without breaking preconditions: [proptest-stateful](https://docs.rs/proptest-stateful/latest/proptest_stateful/)
- Generator bias and slowness: [HATRA 2022](https://harrisongoldste.in/papers/hatra2022.pdf). Uniform generation needed argument bias and fault injection to reach corner states: [ShardStore via Demirbas (secondary)](https://muratbuffalo.blogspot.com/2021/10/using-lightweight-formal-methods-to.html)
- Flaky CI from random discovery: [HATRA 2022](https://harrisongoldste.in/papers/hatra2022.pdf)
- Abandonment for maintenance and staffing reasons: [HATRA 2022](https://harrisongoldste.in/papers/hatra2022.pdf)
- Test code size: the evidence found runs against the "tests larger than code" fear. The dets model was under 100 lines against over 6,000 lines of code, and the Volvo models were 20,000 lines against a million lines of C, an order of magnitude smaller than the equivalent TTCN3 suites. — [Hughes 2016](https://www.cs.tufts.edu/~nr/cs257/archive/john-hughes/quviq-testing.pdf). Those systems had simple intended behavior relative to their implementations, which Hughes notes for dets.

### Inferences
- These failure modes map onto Verbatim's plan as follows:
  - Check a reference model or postconditions, not just invariants.
  - Count how often each operation kind and each interesting state occurs in generated runs. This is a coverage or distribution report: an invariant never violated in states never reached proves nothing.
  - Generate valid operations from the current model state.
  - Fix seeds in CI.
  - Expect a model that duplicates the reducer to add little. A model is useful when it is far simpler than the code, the way a list stands in for dets.
- "Invariants written after the fact" was not found as a named failure in a primary source. The closest evidence is the HATRA finding that developers struggle to imagine properties, and Hughes's observation that errors are often in the model.

### Gaps
- No primary account was found of a team that publicly abandoned stateful PBT, beyond the HATRA manager anecdote.

## Mutation testing as a measure of test strength

### Takeaway
At Google scale, mutation testing works only when it is heavily filtered: one mutant per changed line, suppression of "arid" code, and results shown in code review. With that filtering, developers rated 82% of the mutants they judged as productive, up from about 15% before. This makes it a practical way to answer the effectiveness question the PBT studies leave open.

### Cited Findings
- Google ran the system for more than 24,000 developers on more than 1,000 projects. 16,935,148 mutants were generated across 776,740 changelists, and 2,110,489 were surfaced in review. — [Petrović, Ivanković, Fraser, Just, Practical Mutation Testing at Scale, TSE 2021](https://ar5iv.labs.arxiv.org/html/2102.11378)
- Initially about 85% of reported mutants were judged unproductive. With heuristics, the productive share rose to about 89%. Of the 66,798 mutants given explicit feedback, 82% were labeled productive. — [Petrović et al.](https://ar5iv.labs.arxiv.org/html/2102.11378)
- Median mutants per changelist: 820 with traditional mutation, 77 with one per line, and 7 with arid-node suppression plus one per line. The median number of surviving mutants per changelist was 2. 87.5% of mutants were killed by existing tests. — [Petrović et al.](https://ar5iv.labs.arxiv.org/html/2102.11378)
- An "arid node" is code where mutations are mostly unproductive, such as logging. More than 100 expert-written rules identify them, and some are unsound, accepting the loss of some useful mutants. The ABS operator was dropped as mostly unproductive. Python mutants were less productive (70.6%) than Java (87.2%). — [Petrović et al.](https://ar5iv.labs.arxiv.org/html/2102.11378)
- The paper does not measure correlation with real faults; productivity is judged from developer feedback. Its stated reason for the filtering is that unhelpful mutants would make developers abandon the tool. — [Petrović et al.](https://ar5iv.labs.arxiv.org/html/2102.11378)
- Goldstein et al. name mutation testing and coverage as the standard ways to estimate PBT effectiveness, and still list measuring effectiveness as an open problem. — [HATRA 2022](https://harrisongoldste.in/papers/hatra2022.pdf); [ICSE 2024](https://cis.upenn.edu/~bcpierce/papers/icse24-pbt-in-practice)
- Related research combines the two: [Property-Based Mutation Testing (arXiv 2301.13615)](https://arxiv.org/pdf/2301.13615) (not read).

### Inferences
- A cheap, high-signal check for Verbatim: run cargo-mutants over the reducer crates only, using the same seeded state-machine tests, and treat each surviving mutant as a missing postcondition or a missing generated state. Limit runs to changed code, as Google did, to bound the cost.
- Hughes's injected-bug experiment is mutation testing done by hand, and it is what showed invariant-only checks to be weak. Applying the same method to Verbatim's own generated tests would show whether they are weak in the same way.

### Gaps
- No primary data was retrieved on cargo-mutants' run time or effectiveness, or on PIT industrial case studies. The follow-up Google paper ("Please fix this mutant") was not read.
