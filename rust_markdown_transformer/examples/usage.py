"""rust_markdown_transformer Python 바인딩 최소 사용 예제.

빌드/설치:
    pip install maturin
    maturin develop --release --features python   # 개발용 (현재 venv 에 설치)
    # 또는 배포 휠:
    maturin build --release --features python      # target/wheels/*.whl 생성
    pip install target/wheels/rust_markdown_transformer-*.whl

실행:
    python examples/usage.py <문서경로>
"""

import json
import sys

import rust_markdown_transformer as rmt


def main() -> None:
    if len(sys.argv) < 2:
        print(f"버전: {rmt.__version__}")
        print(f"지원 파서: {rmt.supported_parsers()}")
        print("사용법: python examples/usage.py <문서경로>")
        return

    path = sys.argv[1]
    print(f"지원 여부: {rmt.is_supported(path)}")

    # 1) 포맷 무관 → Markdown
    md = rmt.convert_to_markdown(path)
    print("\n===== Markdown (앞 500자) =====")
    print(md[:500])

    # 2) 벡터 DB 적재용 청크 (heading_path 메타 포함)
    chunks = json.loads(rmt.convert_to_chunks(path, max_tokens=512, overlap=64))
    print(f"\n===== Chunks: {len(chunks)}개 =====")
    for c in chunks[:3]:
        print(f"  heading_path={c['heading_path']}  tokens={c['token_count']}")

    # 3) IR JSON (멀티모달/citation 안전망)
    ir = rmt.convert_to_ir_json(path)
    print(f"\nIR JSON 길이: {len(ir)} bytes")


if __name__ == "__main__":
    main()
