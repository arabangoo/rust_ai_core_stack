# -*- coding: utf-8 -*-
"""PyO3 바인딩 회귀 테스트.

Rust 단위 테스트와 같은 시나리오를 Python 경계에서 다시 주행한다. Rust 쪽이 통과해도
바인딩이 값을 잘못 넘기거나 예외 대신 패닉을 내면 여기서 걸린다.

먼저 확장 모듈을 빌드해야 한다.

    maturin develop --features python
    pytest tests/test_python_binding.py

maturin 없이 확인하려면 cargo 로 cdylib 를 만든 뒤 확장자만 바꿔 경로에 두면 된다.

    cargo build --features python --release
    # Windows:  target/release/rust_pii_transformer.dll  -> rust_pii_transformer.pyd
    # Linux:    target/release/librust_pii_transformer.so -> rust_pii_transformer.so
    # macOS:    target/release/librust_pii_transformer.dylib -> rust_pii_transformer.so
"""
import json

import pytest

rpit = pytest.importorskip(
    "rust_pii_transformer",
    reason="확장 모듈이 빌드되지 않았다. maturin develop --features python 을 먼저 실행한다.",
)


# ── 모듈 표면 ────────────────────────────────────────────────


def test_module_metadata():
    assert rpit.__version__
    # 현재 범위를 감추지 않는다. 층이 바뀌면 이 문자열도 함께 바뀐다.
    assert "detect" in rpit.__status__ and "mask" in rpit.__status__
    assert isinstance(rpit.__has_hash_policy__, bool)


def test_all_four_layers_are_exposed():
    for name in ("normalize", "detect", "mask", "unmask", "entity_names"):
        assert hasattr(rpit, name), "%s 가 노출되지 않았다" % name


def test_insert_is_not_exposed_because_the_core_removed_it():
    # 코어에서 삽입 연산 자체를 걷어냈다. 정규화 비용을 원문 기준으로 매길 수 없기 때문이다.
    assert not hasattr(rpit.SpanMapBuilder(), "insert")


# ── 오프셋 복원 ──────────────────────────────────────────────


def _rrn_builder():
    """880101-1234567 에서 하이픈을 흡수하는 빌더."""
    b = rpit.SpanMapBuilder()
    b.keep("880101")
    b.absorb("-", "separator.hyphen", "separator")
    b.keep("1234567")
    return b


def test_absorbed_separator_inside_span_is_included():
    normalized, smap = _rrn_builder().finish()
    assert normalized == "8801011234567"
    smap.validate()

    src = smap.to_source(rpit.Span(0, 13, 0, 13))
    assert (src.byte_start, src.byte_end) == (0, 14)
    assert src.snapped is False
    assert src.rules == ["separator.hyphen"]
    assert src.cost.absorbed_separators == 1


def test_absorbed_separator_at_boundary_is_excluded():
    b = rpit.SpanMapBuilder()
    b.absorb("-", "separator.hyphen", "separator")
    b.keep("880101")
    normalized, smap = b.finish()
    assert normalized == "880101"

    src = smap.to_source(rpit.Span(0, 6, 0, 6))
    assert (src.byte_start, src.byte_end) == (1, 7), "선행 하이픈은 원문 스팬에 들어가면 안 된다"
    assert src.rules == []


def test_partial_match_inside_expansion_snaps_outward():
    b = rpit.SpanMapBuilder()
    b.numeral("일억", "100000000", "hangul.numeral")
    normalized, smap = b.finish()
    assert normalized == "100000000"
    assert smap.segments[0].kind == "expand"

    src = smap.to_source(rpit.Span(0, 6, 0, 6))
    assert (src.byte_start, src.byte_end) == (0, 6), "'일억' 은 UTF-8 로 6바이트다"
    assert (src.char_start, src.char_end) == (0, 2)
    assert src.snapped is True
    assert src.cost.expanded_syllables == 2


def test_korean_byte_and_char_offsets_stay_separate():
    b = rpit.SpanMapBuilder()
    b.keep("생년월일 ")
    b.numeral("팔팔공일공일", "880101", "hangul.numeral")
    normalized, smap = b.finish()
    assert normalized == "생년월일 880101"

    src = smap.to_source(rpit.Span(13, 19, 5, 11))
    assert (src.byte_start, src.byte_end) == (13, 31), "원문의 '팔팔공일공일' 은 18바이트다"
    assert (src.char_start, src.char_end) == (5, 11)
    assert src.cost.expanded_syllables == 6


def test_source_slice_recovers_the_original_fragment():
    """복원 보장의 핵심. 되돌린 스팬이 원문 조각과 정확히 일치해야 한다."""
    source = "계좌 1234-5678 입금"
    b = rpit.SpanMapBuilder(source)
    b.keep("계좌 1234")
    b.absorb("-", "separator.hyphen", "separator")
    b.keep("5678 입금")
    normalized, smap = b.finish()
    assert normalized == "계좌 12345678 입금"

    start = normalized.index("12345678")
    src = smap.to_source_from_chars(normalized, start, start + 8)
    assert src.span.slice(source) == "1234-5678"


# ── 문자 좌표 경로 (Python 소비자의 기본 경로) ────────────────


def test_char_range_helpers_agree_with_byte_query():
    b = rpit.SpanMapBuilder()
    b.keep("생년월일 ")
    b.numeral("팔팔공일공일", "880101", "hangul.numeral")
    normalized, smap = b.finish()

    start = normalized.index("880101")  # Python 인덱스는 문자 기준이다
    span = rpit.Span.from_char_range(normalized, start, start + 6)
    assert (span.byte_start, span.byte_end) == (13, 19)
    assert smap.to_source(span).byte_end == smap.to_source_from_chars(
        normalized, start, start + 6
    ).byte_end


def test_char_range_accepts_end_of_text():
    span = rpit.Span.from_char_range("가나다", 0, 3)
    assert (span.byte_start, span.byte_end) == (0, 9)


# ── 규칙 목록 ────────────────────────────────────────────────


def test_repeated_rule_is_deduplicated_but_cost_accumulates():
    b = rpit.SpanMapBuilder()
    b.keep("1")
    b.absorb("-", "separator.hyphen", "separator")
    b.keep("2")
    b.absorb("-", "separator.hyphen", "separator")
    b.keep("3")
    _, smap = b.finish()

    src = smap.to_source(rpit.Span(0, 3, 0, 3))
    assert src.rules == ["separator.hyphen"]
    assert src.cost.absorbed_separators == 2


def test_rules_come_out_sorted_by_name_not_application_order():
    b = rpit.SpanMapBuilder()
    b.numeral("팔", "8", "z.numeral")
    b.replace("１", "1", "a.fold")
    b.absorb("-", "m.separator", "separator")
    b.keep("9")
    _, smap = b.finish()

    src = smap.to_source(rpit.Span(0, 4, 0, 4))
    assert src.rules == ["a.fold", "m.separator", "z.numeral"]


# ── 합성 ────────────────────────────────────────────────────


def test_identity_compose_is_a_noop():
    ident = rpit.SpanMap.identity("880101-1234567")
    ident.validate()
    assert ident.is_identity() is True

    _, m2 = _rrn_builder().finish()
    composed = rpit.SpanMap.compose(ident, m2)
    composed.validate()
    assert composed.src_end == ident.src_end
    assert composed.dst_end == m2.dst_end

    src = composed.to_source(rpit.Span(0, 13, 0, 13))
    assert (src.byte_start, src.byte_end) == (0, 14)


def test_compose_rejects_mismatched_coordinate_systems():
    with pytest.raises(rpit.SpanMapError):
        rpit.SpanMap.compose(rpit.SpanMap.identity("abc"), rpit.SpanMap.identity("abcdef"))


# ── 내성과 직렬화 ────────────────────────────────────────────


def test_segment_introspection():
    _, smap = _rrn_builder().finish()
    assert [s.kind for s in smap.segments] == ["identity", "delete", "identity"]
    assert len(smap) == 3

    deleted = smap.segments[1]
    assert deleted.rules == ["separator.hyphen"]
    assert deleted.dst.is_empty() is True
    assert deleted.src.slice("880101-1234567") == "-"
    assert deleted.cost.absorbed_separators == 1


def test_json_export():
    _, smap = _rrn_builder().finish()
    payload = json.loads(smap.to_json())
    assert len(payload["segments"]) == len(smap.segments)

    evidence = json.loads(smap.to_source(rpit.Span(0, 13, 0, 13)).to_json())
    assert evidence["snapped"] is False
    assert evidence["rules"] == ["separator.hyphen"]
    assert evidence["cost"]["absorbed_separators"] == 1


def test_empty_text():
    smap = rpit.SpanMap.identity("")
    smap.validate()
    assert len(smap) == 0
    assert smap.dst_end == (0, 0)
    assert smap.to_source(rpit.Span(0, 0, 0, 0)).span.is_empty() is True


# ── 오류 처리: Rust 패닉이 Python 예외로 바뀌는가 ─────────────


def test_unknown_absorbed_class_raises_value_error():
    with pytest.raises(ValueError, match="unknown absorbed class"):
        rpit.SpanMapBuilder().absorb(" ", "x.rule", "SPACE")


def test_finish_twice_raises_instead_of_panicking():
    b = rpit.SpanMapBuilder()
    b.keep("abc")
    b.finish()
    with pytest.raises(RuntimeError, match="already consumed"):
        b.finish()


def test_use_after_finish_raises_instead_of_panicking():
    b = rpit.SpanMapBuilder()
    b.keep("abc")
    b.finish()
    with pytest.raises(RuntimeError, match="already consumed"):
        b.keep("def")


def test_out_of_range_slice_raises_instead_of_panicking():
    """코어의 Span::slice 는 범위 밖에서 패닉한다. 바인딩이 그것을 막아야 한다."""
    with pytest.raises(ValueError):
        rpit.Span(0, 999, 0, 999).slice("짧다")


def test_non_char_boundary_slice_raises_instead_of_panicking():
    with pytest.raises(ValueError):
        rpit.Span(1, 2, 0, 1).slice("가나")  # '가' 의 중간


def test_char_range_out_of_range_raises():
    with pytest.raises(ValueError):
        rpit.Span.from_char_range("abc", 0, 99)


def test_reversed_char_range_raises():
    with pytest.raises(ValueError):
        rpit.Span.from_char_range("abcdef", 4, 2)


# ── 탐지 층 ──────────────────────────────────────────────────


TEXT = "주민등록번호 팔팔공일공일 - 1234567 이고 연락처는 010-1234-5678 입니다"


def test_detect_finds_a_hangul_numeral_resident_number():
    """이 라이브러리가 존재하는 이유. 한글 수사로 적힌 번호가 원문 좌표로 되돌아온다."""
    report = rpit.detect(TEXT)
    kinds = [f.entity for f in report.findings]
    assert "resident" in kinds
    assert "phone" in kinds

    rrn = next(f for f in report.findings if f.entity == "resident")
    assert rrn.text(TEXT) == "팔팔공일공일 - 1234567"
    assert rrn.entity_label == "주민등록번호"
    assert rrn.evidence.cost.expanded_syllables == 6
    assert any(h.cue == "주민등록번호" for h in rrn.evidence.context_hits)


def test_finding_reports_both_byte_and_char_offsets():
    report = rpit.detect(TEXT)
    f = report.findings[0]
    # 한글이 앞에 있으므로 바이트와 문자 좌표가 달라야 한다.
    assert f.source.byte_start != f.source.char_start
    assert TEXT[f.source.char_start:f.source.char_end] == f.text(TEXT)


def test_certainty_and_checksum_are_lowercase_strings():
    report = rpit.detect("카드번호 4111-1111-1111-1111")
    f = report.findings[0]
    assert f.entity == "credit_card"
    assert f.certainty == "certain"
    assert f.evidence.checksum == "passed"
    assert f.evidence.checksum_reason is None


def test_checksum_reason_is_given_when_not_applicable():
    report = rpit.detect("연락처는 010-1234-5678 입니다")
    f = report.findings[0]
    assert f.evidence.checksum == "not_applicable"
    assert f.evidence.checksum_reason


def test_rejections_explain_why_a_candidate_was_dropped():
    """문맥이 필수인 엔티티가 단서 없이 나오면 no_context 로 떨어진다."""
    report = rpit.detect("첨부 자료 1234567890123 참고하세요")
    assert not report.findings
    reasons = {r.entity: r.reason for r in report.rejections}
    assert reasons["bank_account"] == "no_context"


def test_negative_context_outranks_plain_absence_of_context():
    """부정 문맥 낱말이 있으면 사유가 business_context 로 바뀐다.

    `접수번호` 는 부정 문맥 사전에 있다. 단서가 없는 것(no_context)과 "이건 개인의 것이
    아니다"라고 말하는 낱말이 있는 것(business_context)은 다른 사건이고, 조사할 때
    구분되어야 한다.
    """
    report = rpit.detect("접수번호 1234567890123 입니다")
    assert not report.findings
    reasons = {r.entity: r.reason for r in report.rejections}
    assert reasons["bank_account"] == "business_context"


def test_json_names_match_the_attribute_names():
    """같은 값에 이름이 둘 생기면 안 된다."""
    report = rpit.detect("주민등록번호 880101-1234568")
    f = report.findings[0]
    payload = json.loads(f.to_json())
    assert payload["entity"] == f.entity == "resident"
    assert payload["certainty"] == f.certainty
    assert payload["evidence"]["checksum"] == f.evidence.checksum


def test_entity_names_lists_every_kind():
    names = rpit.entity_names()
    assert "resident" in names and "bank_account" in names and "passport" in names
    assert len(names) == 10


# ── 설정 ─────────────────────────────────────────────────────


def test_raising_the_threshold_filters_results():
    cfg = rpit.Config()
    cfg.min_score = 1.5
    assert len(rpit.detect("연락처는 010-1234-5678 입니다", cfg)) == 0


def test_turning_off_the_hangul_pass_loses_hangul_numerals():
    cfg = rpit.Config()
    cfg.hangul = False
    assert len(rpit.detect("주민등록번호 팔팔공일공일-1234567", cfg)) == 0


def test_turning_off_the_lookalike_pass_loses_substituted_digits():
    cfg = rpit.Config()
    cfg.lookalike = False
    assert rpit.normalize("88O1O1", cfg).text == "88O1O1"
    assert rpit.normalize("88O1O1").text == "880101"


def test_business_words_can_be_disarmed_by_raising_the_veto():
    text = "접수번호 010-1234-5678 로 조회하세요"
    assert len(rpit.detect(text)) == 0

    cfg = rpit.Config()
    cfg.min_veto = 99.0
    assert len(rpit.detect(text, cfg)) > 0


def test_weights_are_readable_and_settable():
    cfg = rpit.Config()
    before = dict(cfg.weights)
    assert "checksum_passed" in before
    cfg.set_weights(context=0.9)
    assert dict(cfg.weights)["context"] == pytest.approx(0.9)
    # 주지 않은 값은 그대로다.
    assert dict(cfg.weights)["checksum_passed"] == pytest.approx(before["checksum_passed"])


def test_normalize_can_be_used_alone():
    out = rpit.normalize("주민등록번호 팔팔공일공일-1234567")
    assert out.text == "주민등록번호 8801011234567"
    out.map.validate()


# ── 마스킹 층 ────────────────────────────────────────────────


def test_default_policy_names_what_was_removed():
    out = rpit.mask("주민등록번호 880101-1234568 입니다")
    assert out.text == "주민등록번호 [주민등록번호] 입니다"
    assert out.restore is None
    assert not out.skipped


def test_per_entity_policies():
    policies = (rpit.PolicySet(rpit.Policy.redact_label())
                .with_entity("phone", rpit.Policy.partial(3, 4)))
    out = rpit.mask("카드 4111-1111-1111-1111 연락처 010-1234-5678", policies)
    assert out.text == "카드 [카드번호] 연락처 010******5678"


def test_fill_policy_preserves_character_count():
    out = rpit.mask("연락처 010-1234-5678", rpit.PolicySet(rpit.Policy.redact_fill("*")))
    assert out.text == "연락처 " + "*" * len("010-1234-5678")


def test_tokenize_round_trips_exactly():
    out = rpit.mask(TEXT, rpit.PolicySet(rpit.Policy.tokenize()))
    assert "팔팔공일공일" not in out.text
    assert rpit.unmask(out.text, out.restore) == TEXT


def test_restore_map_survives_a_json_round_trip():
    """프로세스가 끝나도 되돌릴 수 있어야 가역이라 부를 수 있다."""
    out = rpit.mask(TEXT, rpit.PolicySet(rpit.Policy.tokenize()))
    reloaded = rpit.RestoreMap.from_json(out.restore.to_json())
    assert len(reloaded) == len(out.restore)
    assert rpit.unmask(out.text, reloaded) == TEXT


def test_bytes_outside_findings_are_untouched():
    text = "앞 문장. 연락처 010-1234-5678 뒤 문장."
    for policy in (rpit.Policy.redact_label(),
                   rpit.Policy.redact_fill("*"),
                   rpit.Policy.partial(3, 4),
                   rpit.Policy.tokenize()):
        out = rpit.mask(text, rpit.PolicySet(policy))
        assert out.text.startswith("앞 문장. 연락처 ")
        assert out.text.endswith(" 뒤 문장.")


def test_text_without_pii_is_returned_verbatim():
    text = "이번 분기 실적은 전년 대비 개선되었습니다."
    out = rpit.mask(text, rpit.PolicySet(rpit.Policy.tokenize()))
    assert out.text == text
    assert out.restore.is_empty()


def test_hash_policy_links_equal_values():
    if not rpit.__has_hash_policy__:
        pytest.skip("hash 기능 없이 빌드된 확장이다")
    out = rpit.mask("연락처 010-1234-5678 과 010-1234-5678",
                    rpit.PolicySet(rpit.Policy.hash(b"secret", 12)))
    # 해시 토큰의 접두어는 기계가 짝을 맞추는 값이라 영문 식별자를 쓴다.
    tokens = [part for part in out.text.split() if part.startswith("[PHONE:")]
    assert len(tokens) == 2 and tokens[0] == tokens[1]


# ── 산출물 언어 ──────────────────────────────────────────────


def test_code_policy_keeps_placeholders_ascii():
    """한국어 문서를 다루는 비한국어 사용자의 경로. 자리표시자에 한글이 없어야 한다."""
    out = rpit.mask("카드 4111-1111-1111-1111 연락처 010-1234-5678",
                    rpit.PolicySet(rpit.Policy.redact_code()))
    assert out.text == "카드 [CREDIT_CARD] 연락처 [PHONE]"


def test_label_and_code_name_the_same_entity():
    text = "주민등록번호 880101-1234568"
    korean = rpit.mask(text, rpit.PolicySet(rpit.Policy.redact_label()))
    english = rpit.mask(text, rpit.PolicySet(rpit.Policy.redact_code()))
    assert korean.text == "주민등록번호 [주민등록번호]"
    assert english.text == "주민등록번호 [RESIDENT]"
    assert korean.applied[0].entity == english.applied[0].entity == "resident"


def test_entity_exposes_all_three_names():
    f = rpit.detect("카드번호 4111-1111-1111-1111").findings[0]
    assert f.entity == "credit_card"          # 기계용
    assert f.entity_label == "카드번호"        # 한국어
    assert f.entity.upper() == "CREDIT_CARD"  # 자리표시자에 쓰이는 영문


# ── 오류 경로 ────────────────────────────────────────────────


def test_unknown_entity_name_raises_value_error():
    with pytest.raises(ValueError) as err:
        rpit.PolicySet().with_entity("nope", rpit.Policy.tokenize())
    assert "unknown entity" in str(err.value)


def test_unknown_restore_token_raises_instead_of_silently_passing():
    out = rpit.mask("연락처 010-1234-5678", rpit.PolicySet(rpit.Policy.tokenize()))
    with pytest.raises(rpit.SpanMapError):
        rpit.unmask("[[PII:99]]", out.restore)


def test_malformed_restore_map_json_raises():
    with pytest.raises(ValueError):
        rpit.RestoreMap.from_json("{not json")