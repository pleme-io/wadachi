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
;;; Combine → FloorIndexed → MatchNeedle → SortDesc → CollapseDescendants →
;;; TopK.
;;;
;;; The Combine phase folds the decayed weights and the visit count into one
;;; score, and HOW it folds them is the :combine clause — a named variant, not
;;; a fixed expression:
;;;
;;;   RecencySumPlusFreq    recency-weight · Σ decay(age_days(visit))
;;;                       + freq-weight    · visit-count
;;;   FreqTimesLatestDecay  recency-weight · visit-count
;;;                                        · decay(age_days(last visit))
;;;
;;; …then scaled by match-weight(best-match-kind(needle, path)).
;;; discovered-only entries are floored to :indexed-epsilon by FloorIndexed.
;;; :combine is optional and defaults to RecencySumPlusFreq, which is what the
;;; interpreter did unconditionally before the variant was modeled.
;;;
;;; FreqTimesLatestDecay does not read :freq-weight — frequency enters
;;; multiplicatively, so there is no additive frequency term for it to scale.
;;; Instances selecting it write :freq-weight 0.0 to say so rather than leave a
;;; live-looking knob that does nothing.
;;;
;;; MATCHING IS PART OF THE SPEC. It did not used to be: each consumer
;;; pre-filtered with its own `path.contains(needle)` before the interpreter
;;; ran, which no spec governed and no matrix tested. That is why `cd ni`
;;; answered with `…/akeyless-commu-NI-ty/…` — a coincidence in the middle of
;;; an ancestor's name, scoring identically to a directory actually lived in.
;;; The `:match` clause below is the typed replacement.
;;;
;;; Match kinds, worst → best (the order IS the `:require-at-least` ladder):
;;;   SubstringAnywhere     needle occurs anywhere in the full path
;;;   ComponentPrefix       a non-final component starts with the needle
;;;   BasenameSubsequence   needle's chars occur in order in the basename
;;;   BasenamePrefix        the basename starts with the needle
;;;   BasenameExact         the basename IS the needle
;;;
;;; A needle containing `/` is read as ordered path fragments: each leading
;;; fragment must match an ancestor component in order, and the final fragment
;;; is classified against the basename — so `cd pleme-io/wad` means what an
;;; operator expects.

;; The fleet default: Σ 1/(1+age_days), recency-only, anchored matching.
;; The score formula is behavior-identical to skim-tab's historical
;; `frecency_score`, which makes adopting this spec in skim-tab a
;; behavior-preserving extraction.
(deffrecency-ranking
  :name "skimtab-parity"
  :decay HyperbolicDays
  :half-life-days 0.0
  :freq-weight 0.0
  :recency-weight 1.0
  :indexed-epsilon 0.001
  ;; Anchored: the basename is what an operator is naming, so it dominates —
  ;; and SubstringAnywhere is excluded outright rather than down-weighted,
  ;; because a coincidence in the middle of a word is not a weak match, it is
  ;; not a match.
  :match (:basename-exact 8.0 :basename-prefix 4.0 :basename-subsequence 1.0
          :component-prefix 0.5 :substring-anywhere 0.1
          :require-at-least ComponentPrefix)
  :phases ((:kind LoadEntries) (:kind ComputeAge) (:kind ApplyDecay)
           (:kind Combine) (:kind FloorIndexed) (:kind MatchNeedle)
           (:kind SortDesc) (:kind CollapseDescendants :keep 0)
           (:kind TopK :n 50)))

;; praça's session ranking: visit-count × the bucket its LAST visit falls in.
;; This — not "zoxide-parity" below — is what zoxide actually computes.
;;
;; It is a named instance because the alternative already happened: praça needed
;; a multiplicative combine, the spec could only express the additive one, and
;; tear/praca/src/frecency.rs hand-copied the ZoxideLogBuckets thresholds
;; (1h/1d/1w) and multipliers (4.0/2.0/0.5/0.25) into a crate that did not
;; depend on this one. A combine a consumer cannot select is a defect HERE.
(deffrecency-ranking
  :name "praca-parity"
  :decay ZoxideLogBuckets
  :half-life-days 0.0
  ;; Unread under this combine — see the header note.
  :freq-weight 0.0
  :recency-weight 1.0
  :combine FreqTimesLatestDecay
  :indexed-epsilon 0.001
  :match (:basename-exact 8.0 :basename-prefix 4.0 :basename-subsequence 1.0
          :component-prefix 0.5 :substring-anywhere 0.1
          :require-at-least ComponentPrefix)
  :phases ((:kind LoadEntries) (:kind ComputeAge) (:kind ApplyDecay)
           (:kind Combine) (:kind FloorIndexed) (:kind MatchNeedle)
           (:kind SortDesc) (:kind CollapseDescendants :keep 0)
           (:kind TopK :n 50)))

;; zoxide muscle-memory: frequency × exponential half-life. (Despite the name,
;; this is NOT zoxide's real formula — see "praca-parity" above, which is.)
(deffrecency-ranking
  :name "zoxide-parity"
  :decay ExpHalfLife
  :half-life-days 30.0
  :freq-weight 1.0
  :recency-weight 1.0
  :indexed-epsilon 0.001
  :match (:basename-exact 8.0 :basename-prefix 4.0 :basename-subsequence 1.0
          :component-prefix 0.5 :substring-anywhere 0.1
          :require-at-least ComponentPrefix)
  :phases ((:kind LoadEntries) (:kind ComputeAge) (:kind ApplyDecay)
           (:kind Combine) (:kind FloorIndexed) (:kind MatchNeedle)
           (:kind SortDesc) (:kind CollapseDescendants :keep 0)
           (:kind TopK :n 50)))

;; The pre-spec behavior, preserved as a SELECTABLE instance rather than
;; deleted (MODULARIZE, DON'T DELETE): every match kind weighted equally,
;; nothing excluded, no descendant collapse. Selecting this returns the exact
;; feel wadachi shipped before matching was modeled — and the verification
;; matrix pins the difference between it and `skimtab-parity`, so "what
;; changed" is a test, not a changelog claim.
(deffrecency-ranking
  :name "substring-legacy"
  :decay HyperbolicDays
  :half-life-days 0.0
  :freq-weight 0.0
  :recency-weight 1.0
  :indexed-epsilon 0.001
  :match (:basename-exact 1.0 :basename-prefix 1.0 :basename-subsequence 1.0
          :component-prefix 1.0 :substring-anywhere 1.0
          :require-at-least SubstringAnywhere)
  :phases ((:kind LoadEntries) (:kind ComputeAge) (:kind ApplyDecay)
           (:kind Combine) (:kind FloorIndexed) (:kind MatchNeedle)
           (:kind SortDesc) (:kind TopK :n 50)))
