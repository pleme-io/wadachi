;;; frecency.lisp — the spec-of-record for wadachi's frecency ranking.
;;;
;;; This is the authored Lisp leg of the TYPED-SPEC + INTERPRETER TRIPLET.
;;; Each (deffrecency-ranking …) form declares ONE named instance of the one
;;; ranking algorithm as data; the typed border lives in
;;; wadachi-spec/src/spec.rs and the interpreter in src/interp.rs walks the
;;; phases. The Rust constructors `FrecencyRankingSpec::skimtab_parity()` /
;;; `::zoxide_parity()` mirror these forms field-for-field — until the
;;; `#[derive(DeriveTataraDomain)]` keyword wiring lands (fast-follow), these
;;; forms are the human-readable contract those constructors must match.
;;;
;;; Phases (in canonical order): LoadEntries → ComputeAge → ApplyDecay →
;;; Combine → FloorIndexed → SortDesc → TopK.
;;;
;;; score(entry) = recency-weight · Σ decay(age_days(visit))
;;;             + freq-weight    · visit-count
;;; discovered-only entries are floored to :indexed-epsilon by FloorIndexed.

;; The fleet default: Σ 1/(1+age_days), recency-only. Behavior-identical to
;; skim-tab's historical `frecency_score`, which makes adopting this spec in
;; skim-tab a behavior-preserving extraction.
(deffrecency-ranking
  :name "skimtab-parity"
  :decay HyperbolicDays
  :half-life-days 0.0
  :freq-weight 0.0
  :recency-weight 1.0
  :indexed-epsilon 0.001
  :phases ((:kind LoadEntries) (:kind ComputeAge) (:kind ApplyDecay)
           (:kind Combine) (:kind FloorIndexed) (:kind SortDesc)
           (:kind TopK :n 50)))

;; zoxide muscle-memory: frequency × exponential half-life.
(deffrecency-ranking
  :name "zoxide-parity"
  :decay ExpHalfLife
  :half-life-days 30.0
  :freq-weight 1.0
  :recency-weight 1.0
  :indexed-epsilon 0.001
  :phases ((:kind LoadEntries) (:kind ComputeAge) (:kind ApplyDecay)
           (:kind Combine) (:kind FloorIndexed) (:kind SortDesc)
           (:kind TopK :n 50)))
