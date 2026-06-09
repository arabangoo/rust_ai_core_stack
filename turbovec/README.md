# turbovec — 학습이 필요 없는 Rust 벡터 인덱스

Google Research가 ICLR 2026에서 공개한 **TurboQuant** 알고리즘을 Rust로 구현하고 Python 바인딩으로 노출한 벡터 인덱스(vector index) 라이브러리.

> 1,000만 건 규모의 임베딩(embedding)을 float32로 저장하면 31GB가 필요하지만, turbovec은 같은 데이터를 4GB에 담으면서 FAISS보다 빠르게 검색한다. 별도의 코드북(codebook) 학습이나 캘리브레이션 패스 없이, 벡터를 그대로 인덱스에 추가하기만 하면 된다.

관련 링크:

- 논문: TurboQuant (ICLR 2026) — arxiv.org/abs/2504.19874
- 보정 기법 출처: RaBitQ (SIGMOD 2024) — arxiv.org/abs/2405.12497
- GitHub: github.com/RyanCodrai/turbovec
- 패키지: PyPI `turbovec` · crates.io `turbovec`

---

## 목차

1. [핵심 요약](#핵심-요약)
2. [왜 turbovec 인가](#왜-turbovec-인가)
3. [설치](#설치)
4. [빠른 시작](#빠른-시작)
5. [알고리즘 작동 원리](#알고리즘-작동-원리)
6. [API 레퍼런스](#api-레퍼런스)
7. [필터링 (하이브리드 검색)](#필터링-하이브리드-검색)
8. [파일 포맷](#파일-포맷)
9. [성능](#성능)
10. [프레임워크 통합](#프레임워크-통합)
11. [빌드](#빌드)
12. [벤치마크 실행](#벤치마크-실행)
13. [인코딩 비용과 성능 특성](#인코딩-비용과-성능-특성)
14. [한계와 주의사항](#한계와-주의사항)
15. [라이선스](#라이선스)
16. [참고 문헌](#참고-문헌)

---

## 핵심 요약

| 항목      | 내용                                                                                       |
| --------- | ------------------------------------------------------------------------------------------ |
| 정체      | TurboQuant 알고리즘의 Rust 구현 + Python 바인딩 벡터 인덱스                                |
| 차별점    | 데이터 무관형(data-oblivious) 양자화 — 코드북 학습·train 단계 없음                       |
| 압축      | 1,536차원 float32 벡터(6,144바이트)를 2비트 384바이트(약 16배) / 4비트 768바이트(약 8배)로 |
| 메모리    | 1,000만 건 코퍼스 약 31GB(float32) → 2비트 양자화로 약 4GB                                |
| 속도      | ARM에서 FAISS FastScan 대비 12-20% 빠름, x86에서 대등하거나 우위                           |
| 회복률    | OpenAI 1536/3072차원에서 R@1 기준 FAISS 대비 0.4-3.4점 우위                                |
| 동작 위치 | 순수 로컬 — 외부 API 호출 없음, 완전 에어갭(air-gapped) 구성 가능                         |
| 라이선스  | MIT                                                                                        |

여기서 SIMD(단일 명령 다중 데이터 병렬 연산), R@1(Recall@1, 정답 1건을 상위 1위에서 맞히는 비율), RAG(검색 증강 생성)를 가리킨다. 이후 본문에서는 약자만 쓴다.

---

## 왜 turbovec 인가

기존 FAISS의 곱 양자화(Product Quantization, PQ)는 **데이터 의존적**이다. 코퍼스에 대해 k-means 코드북을 먼저 학습해야 하고, 데이터가 늘면 재학습·재빌드가 필요하다.

turbovec(TurboQuant)은 **데이터 무관형**이다.

- **온라인 인제스트** — 벡터를 add 하면 그 즉시 인덱싱된다. train 단계도, 파라미터 튜닝도, 코퍼스 증가에 따른 재빌드도 없다.
- **FAISS보다 빠름** — ARM은 NEON(ARM SIMD 명령셋), x86은 AVX-512BW(x86 512비트 SIMD 명령셋) 커널을 직접 작성해 FAISS IndexPQFastScan 대비 ARM 12-20% 우위, x86 대등 이상.
- **검색 시점 필터링** — 허용 id 목록(allowlist)이나 슬롯 비트마스크를 `search()`에 넘기면 커널이 그것을 직접 반영한다. 허용 집합에서 최대 `k`건을 항상 받으므로, 과다 조회(over-fetch)도 선택적 필터에서의 회복률 손실도 없다.
- **순수 로컬** — 관리형 서비스 없음, 데이터가 머신·가상 사설망 밖으로 나가지 않음. 오픈소스 임베딩 모델과 결합하면 완전 에어갭 RAG 스택을 구성한다.

프라이버시·메모리·지연시간이 중요한 RAG를 만든다면 적합한 선택지다.

---

## 설치

### Python

```bash
pip install turbovec
```

프레임워크 통합 어댑터까지 함께:

```bash
pip install turbovec[langchain]
pip install turbovec[llama-index]
pip install turbovec[haystack]
pip install turbovec[agno]
```

### Rust

```bash
cargo add turbovec
```

x86_64 빌드는 `.cargo/config.toml`을 통해 `x86-64-v3`(AVX2 기준선, Haswell 2013년 이후)을 타깃으로 한다. AVX2 폴백 커널을 돌릴 수 있는 CPU면 전체 크레이트가 동작하며, AVX-512 커널은 런타임에 `is_x86_feature_detected!` 매크로로 감지해 지원 하드웨어에서만 켜진다.

---

## 빠른 시작

### Python — 기본 인덱스 (TurboQuantIndex)

```python
from turbovec import TurboQuantIndex

index = TurboQuantIndex(dim=1536, bit_width=4)
index.add(vectors)          # np.ndarray, shape (n, dim), float32
index.add(more_vectors)

scores, indices = index.search(query, k=10)

index.write("my_index.tv")
loaded = TurboQuantIndex.load("my_index.tv")
```

### Python — 안정적 외부 id (IdMapIndex)

삭제가 일어나도 외부 id가 그대로 유지돼야 하면 `IdMapIndex`를 쓴다.

```python
import numpy as np
from turbovec import IdMapIndex

index = IdMapIndex(dim=1536, bit_width=4)
index.add_with_ids(vectors, np.array([1001, 1002, 1003], dtype=np.uint64))

scores, ids = index.search(query, k=10)   # ids는 사용자가 부여한 uint64 외부 id
index.remove(1002)                          # id 기준 O(1) 삭제

index.write("my_index.tvim")
loaded = IdMapIndex.load("my_index.tvim")
```

### Python — 하이브리드 검색 (allowlist)

다른 시스템(SQL, BM25(어휘 매칭 검색 알고리즘), 접근 제어 목록(ACL), 시간 윈도우 등)이 만든 후보 집합으로 결과를 제한한다.

```python
import numpy as np
from turbovec import IdMapIndex

idx = IdMapIndex(dim=1536, bit_width=4)
idx.add_with_ids(vectors, ids)

# 1단계: 외부 시스템이 후보 id로 좁힌다.
allowed = np.array(
    db.execute("SELECT id FROM docs WHERE tenant=?", (t,)).fetchall(),
    dtype=np.uint64,
)

# 2단계: 후보 집합 안에서 밀집 벡터 재정렬(dense rerank).
scores, ids = idx.search(query, k=10, allowlist=allowed)
```

필터링은 32-벡터 블록 단위로 SIMD 커널 내부에서 일어난다. 허용 슬롯이 하나도 없는 블록은 룩업 테이블 조회나 점수 계산 전에 단락(short-circuit)되고, 점수가 매겨진 블록 안의 비허용 슬롯은 힙 삽입 시점에 버려진다. 따라서 선택적 allowlist(인덱스의 작은 일부만 허용)는 SIMD 비용 대부분을 치르지 않는다. 출력 길이는 `min(k, len(allowed))`로, allowlist가 `k`보다 작으면 패딩 없이 정확히 `len(allowed)`건을 돌려준다.

### Rust

```rust
use turbovec::TurboQuantIndex;

let mut index = TurboQuantIndex::new(1536, 4);
index.add(&vectors);
let results = index.search(&queries, 10);
index.write("index.tv").unwrap();
let loaded = TurboQuantIndex::load("index.tv").unwrap();
```

삭제에도 안정적인 외부 id가 필요하면:

```rust
use turbovec::IdMapIndex;

let mut index = IdMapIndex::new(1536, 4);
index.add_with_ids(&vectors, &[1001, 1002, 1003]);
let (scores, ids) = index.search(&queries, 10);
index.remove(1002);
index.write("index.tvim").unwrap();
let loaded = IdMapIndex::load("index.tvim").unwrap();
```

---

## 알고리즘 작동 원리

각 벡터는 고차원 초구(hypersphere) 위의 한 방향이다. TurboQuant은 단순한 통찰로 이 방향을 압축한다. 무작위 회전을 적용하면 모든 좌표가 입력 데이터와 무관하게 알려진 분포를 따른다는 것이다.

가장 먼저, **정규화**. 각 벡터에서 길이(norm)를 떼어내 float 하나로 저장한다. 이제 모든 벡터는 초구 위의 단위 방향이 된다.

다음, **무작위 직교 회전**. 모든 벡터에 같은 무작위 직교 행렬을 곱한다. 회전 후 각 좌표는 독립적으로 Beta 분포를 따르며, 고차원에서 Gaussian N(0, 1/d)로 수렴한다. 이는 어떤 입력 데이터에서도 성립한다 — 회전이 좌표 분포를 예측 가능하게 만든다.

세 번째, **좌표별 캘리브레이션(TQ+)**. 위 Beta 분포는 점근적이라 유한 차원에서는 개별 좌표가 표준 형태에서 벗어난다(특히 저비트·단어 벡터류 임베딩). TQ+는 첫 add 때 좌표마다 시프트·스케일 두 스칼라를 맞춰, 각 좌표의 실측 5/95% 분위수를 표준 Beta 주변 분포에 매핑한다. 그러면 Lloyd-Max 코드북이 설계된 목표 분포에 대해 양자화한다. 이 캘리브레이션은 첫 add 후 고정돼 이후 add에 재사용된다 — 재학습·재빌드·별도 train 단계가 없다. 회복률 이득은 가장 많이 드리프트하는 셀에서 R@1 기준 최대 +1.4%포인트(예: GloVe 2비트).

네 번째, **Lloyd-Max 스칼라 양자화**. 분포가 알려져 있으므로 각 좌표를 버킷으로 나누는 최적 방식을 미리 계산할 수 있다(2비트는 4버킷, 4비트는 16버킷). Lloyd-Max 알고리즘이 평균 제곱 오차를 최소화하는 버킷 경계와 중심값을 찾는다. 이는 데이터가 아니라 수학에서 한 번만 계산된다.

다섯 번째, **비트 패킹**. 이제 각 좌표는 작은 정수(2비트는 0-3, 4비트는 0-15)다. 이를 바이트에 빽빽이 채운다. 1,536차원 벡터는 6,144바이트(float32)에서 384바이트(2비트)로 줄어든다 — 16배 압축.

마지막, **길이 재정규화 스코어링**. 스칼라 양자화는 내적을 체계적으로 과소평가한다(복원된 단위 방향이 원본보다 조금 짧다). 인코딩 시점에 벡터마다 스칼라 하나(회전된 단위 벡터와 그 자신의 중심값 복원본의 내적)를 계산해, 압축 벡터 옆에 `||v|| / ⟨u, x̂⟩`를 저장한다. 검색 커널이 후보별 점수에 이 스칼라를 곱한 뒤 힙에 삽입하므로, 내적 추정기는 검색 시점 비용 0과 추가 저장 0으로 하향 편향에서 무편향이 된다. 회복률 이득은 양자화 수축이 가장 큰 저비트 폭에서 가장 크게 나타난다.

검색 시점에는 데이터베이스 벡터를 복원하지 않는다. 쿼리를 같은 영역으로 한 번 회전시킨 뒤 코드북 값에 대해 직접 점수를 매긴다. 스코어링 커널은 SIMD 명령(ARM은 NEON, 최신 x86은 AVX-512BW, 그 외는 AVX2 폴백)과 니블 분할 룩업 테이블(nibble-split lookup table)로 처리량을 극대화한다.

Lloyd-Max 코드북은 정보 이론적 하한(Shannon의 왜곡-비트율 한계) 대비 2.7배 이내의 왜곡에 도달하며, 길이 재정규화 단계가 Lloyd-Max 코드북이 내적 추정기에 남기는 잔여 편향을 제거한다.

---

## API 레퍼런스

turbovec은 인덱스 타입 두 종류와 타입별 직렬화 포맷 하나씩을 노출한다.

- `TurboQuantIndex` — 위치 기반 인덱스, O(1) `swap_remove` 삭제
- `IdMapIndex` — `TurboQuantIndex` 위에 안정적 외부 `u64` id를 얹은 타입

아래 예시는 모두 Python이다. Rust API도 동일한 형태이며 정확한 시그니처는 각 타입의 rustdoc을 참조한다.

### TurboQuantIndex

위치 기반 인덱스. 각 벡터는 삽입 슬롯(`0..n`)으로 식별된다. 빠르고 작지만, 슬롯에 대한 외부 참조는 `swap_remove`로 무효화된다. 안정적 id가 필요하면 `IdMapIndex`를 쓴다.

`dim`은 선택값이다. 생략하면 첫 벡터 배치에서 차원을 잡는다.

```python
idx = TurboQuantIndex(bit_width=4)      # 첫 add 때 dim 추론
idx.add(vectors)                         # dim을 vectors.shape[1]로 고정
```

첫 add 전에는 `idx.dim`이 `None`, `len(idx)`가 `0`이고, `search()`는 빈 결과를 돌려준다.

메서드:

| 메서드                                         | 설명                                                                                                                                                                                                                       |
| ---------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `TurboQuantIndex(dim=None, bit_width=4)`     | `bit_width`는 2 또는 4. `dim`은 선택값이며 생략 시 첫 `add`에서 추론                                                                                                                                                 |
| `add(vectors)`                               | `vectors`는 shape `(n, dim)`의 연속 float32 배열. 지연(lazy) 인덱스의 첫 호출이 `dim`을 고정하고 이후 호출은 일치해야 함. 차원 불일치 시 `ValueError`                                                              |
| `search(queries, k, *, mask=None)`           | `(scores, indices)` 반환, 둘 다 shape `(nq, effective_k)`. `indices`는 int64 슬롯 위치. `mask`는 길이 `len(idx)`의 bool 배열로, 주어지면 `mask[i] == True`인 슬롯만 기여. `effective_k = min(k, mask.sum())` |
| `swap_remove(idx)`                           | O(1). 마지막 벡터를 `idx` 슬롯으로 옮기고, 옮겨진 벡터의 이전 위치를 반환(외부 참조 갱신용)                                                                                                                              |
| `prepare()`                                  | 선택. 회전 행렬·Lloyd-Max 중심값·SIMD 블록 레이아웃을 미리 구성해 첫 `search`의 일회성 비용을 제거. 첫 add 전 지연 인덱스에서는 no-op                                                                                  |
| `write(path)` / `load(path)`               | `.tv` 포맷                                                                                                                                                                                                               |
| `len(idx)` / `idx.dim` / `idx.bit_width` | 조회.`idx.dim`은 확정 후 `int`, 첫 add 전 지연 인덱스에서는 `None`                                                                                                                                                   |

`swap_remove` 의미: Rust의 `Vec::swap_remove`와 같은 이름이다. 마지막 원소가 슬롯 `i`로 들어오고 벡터는 하나 줄어든다. 이는 시프트(FAISS `IndexPQ::remove_ids` 동작)가 **아니다**. 순서가 보존되지 않으며, 삭제하지 않은 벡터의 슬롯 인덱스가 이전과 다른 벡터를 가리킬 수 있다. 외부 참조가 삭제에도 안정적이어야 하면 `IdMapIndex`를 쓴다.

### IdMapIndex

`TurboQuantIndex`를 감싸는 안정적 id 래퍼. FAISS `IndexIDMap2`에 대략 대응하며 해시테이블 기반 O(1) `remove(id)`를 제공한다.

```python
import numpy as np
from turbovec import IdMapIndex

idx = IdMapIndex(dim=1536, bit_width=4)
idx.add_with_ids(vectors, np.array([1001, 1002, 1003], dtype=np.uint64))

scores, ids = idx.search(queries, k=10)   # ids는 uint64 외부 id
idx.remove(1002)                           # id 기준 O(1)
assert 1003 in idx                         # __contains__ 지원

idx.write("index.tvim")
loaded = IdMapIndex.load("index.tvim")
```

메서드:

| 메서드                                                         | 설명                                                                                                                                                                                                                          |
| -------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `IdMapIndex(dim=None, bit_width=4)`                          | `dim`은 선택값이며 생략 시 첫 `add_with_ids`에서 추론                                                                                                                                                                     |
| `add_with_ids(vectors, ids)`                                 | `ids`는 길이 `vectors.shape[0]`의 uint64 배열. 첫 호출이 `dim` 고정. 차원 불일치·중복 id·길이 불일치 시 `ValueError`                                                                                                |
| `remove(id) -> bool`                                         | 있으면 제거 후 `True`, 없으면 `False`. O(1)                                                                                                                                                                               |
| `search(queries, k, *, allowlist=None)`                      | `(scores, ids)` 반환, `ids`는 uint64 외부 id. `allowlist`는 선택적 uint64 id 배열로, 주어지면 결과가 그 id로 제한되고 `effective_k = min(k, len(allowlist))`. 빈 allowlist는 `ValueError`, 미지의 id는 `KeyError` |
| `contains(id)` / `id in idx`                               | 멤버십                                                                                                                                                                                                                        |
| `write(path)` / `load(path)`                               | `.tvim` 포맷                                                                                                                                                                                                                |
| `len(idx)` / `idx.dim` / `idx.bit_width` / `prepare()` | `TurboQuantIndex`와 동일                                                                                                                                                                                                    |

언제 어느 것을 쓰는가:

- `TurboQuantIndex` — 삭제를 하지 않거나, 위치 기반 id로 충분한 경우
- `IdMapIndex` — 안정적 외부 id가 필요한 경우(예: 호출자가 유지하는 문자열-id → 벡터 매핑)

프레임워크 통합(LangChain, LlamaIndex, Haystack)은 모두 이 이유로 내부에서 `IdMapIndex`를 쓴다.

---

## 필터링 (하이브리드 검색)

두 인덱스 타입 모두 반환되는 상위 `k`를 호출자가 준 부분집합으로 제한할 수 있다. 후처리 필터링(검색 후 버리기)과 달리, 커널이 비허용 벡터를 쿼리별 힙에 애초에 삽입하지 않으므로 더 적게가 아니라 허용 집합에서 최대 `k`건을 항상 받는다.

```python
# IdMapIndex — 외부 id allowlist (전형적 사용)
allowed = np.array([1003, 1010, 1042], dtype=np.uint64)
scores, ids = idx.search(queries, k=10, allowlist=allowed)
# scores.shape == (nq, min(k, len(allowed))) == (nq, 3)

# TurboQuantIndex — 슬롯에 대한 bool 마스크
mask = np.ones(len(idx), dtype=bool)
mask[disabled_slots] = False
scores, slots = idx.search(queries, k=10, mask=mask)
```

출력 shape은 `(nq, min(k, n_allowed))`로, `k > len(idx)`일 때 이미 보이는 축소 동작과 같다. `-1` 또는 `NaN` 패딩은 없으니, 고정 너비 배치가 필요하면 호출자 쪽에서 패딩한다.

대표 활용:

- SQL/BM25 단계가 후보 id 집합을 만드는 하이브리드 검색
- 접근 제어 또는 다중 테넌트 쿼리(호출자가 볼 수 있는 id만 반환)
- 시간 윈도우 검색(예: 최근 7일 문서만)

---

## 파일 포맷

### `.tv` — TurboQuantIndex

- 9바이트 헤더
  - `bit_width` (u8)
  - `dim` (u32 리틀엔디안)
  - `n_vectors` (u32 리틀엔디안)
- 패킹된 코드 — `(dim / 8) * bit_width * n_vectors` 바이트
- norms — `n_vectors`개의 f32 리틀엔디안

### `.tvim` — IdMapIndex

- 매직 `"TVIM"` (4바이트)
- 버전 u8 = 1
- 코어 페이로드 (`.tv`와 동일)
- `slot_to_id` — `n_vectors`개의 u64 리틀엔디안

로드 시 역방향 `id → slot` 맵을 메모리에서 재구성한다. `slot_to_id` 테이블의 중복 id는 손상으로 보고 거부된다. 헤더의 `dim = 0`은 아직 확정되지 않은 지연 인덱스를 뜻한다(생성자가 `dim ≥ 8`을 단언하므로 이 값은 모호하지 않다). `dim = 0`은 `n_vectors = 0`과 함께일 때만 유효하며, 로드 시 첫 `add` / `add_with_ids` 호출 전까지 `dim`이 `None`인 인덱스가 된다.

두 포맷 모두 마이너 버전 간 안정적이다. 호환성을 깨는 변경은 파일 포맷 버전 바이트(`.tvim`)나 헤더 길이(`.tv`)를 올린다.

---

## 성능

### 회복률 (Recall)

TurboQuant 대 FAISS `IndexPQ`(LUT256, nbits=8) 비교 — 논문 4.4절 기준선. 10만 벡터, k=64. FAISS PQ 서브양자화기 수를 TurboQuant 비트율에 맞춤(2비트에서 m=d/4, 4비트에서 m=d/2).

OpenAI 1536차원과 3072차원에서 TurboQuant은 2비트·4비트 R@1 기준 FAISS 대비 0.4-3.4점 앞서고, 둘 다 k=4에서 1.0으로 수렴한다. GloVe 200차원은 더 어려운 구간이다(저차원에서 점근적 Beta 가정이 느슨하다). 여기서 TurboQuant은 4비트 R@1에서 0.3점 앞서고 2비트에서 1.2점 뒤지며, 둘 다 k≈16에서 FAISS에 수렴한다.

기준선에 관하여: 비교 대상으로 FAISS `IndexPQ`(LUT256, nbits=8, float32 LUT)를 쓴 이유는 대부분의 사용자가 실제 운영에서 선택할 기본 곱 양자화이기 때문이다. 이는 TurboQuant 논문의 u8-LUT PQ보다 강한 기준선이다(FAISS는 스코어링 시 더 높은 정밀도의 LUT와 k-means++ 코드북 학습을 쓴다). GloVe에서 보이는 격차는 FAISS가 강한 기준선이라는 뜻이지 TurboQuant 구현 문제가 아니다.

### 압축

- 2비트: 1,536차원 6,144바이트 → 384바이트 (약 16배)
- 4비트: 1,536차원 6,144바이트 → 768바이트 (약 8배)

### 검색 속도

모든 벤치마크: 10만 벡터, 1천 쿼리, k=64, 5회 실행 중앙값.

ARM (Apple M3 Max): 모든 설정에서 TurboQuant이 FAISS FastScan 대비 12-20% 빠르다.

x86 (Intel Xeon Platinum 8481C / Sapphire Rapids, 8 vCPU): 모든 4비트 설정에서 1-6% 우위, 2비트 단일 스레드는 FAISS 대비 ±1% 이내. 2비트 멀티 스레드(1536·3072차원)만 FAISS보다 2-4% 뒤지는 유일한 구간으로, 내부 누적 루프가 너무 짧아 언롤링 분할 상환이 FAISS의 AVX-512 VBMI 경로를 따라가지 못한다.

---

## 프레임워크 통합

각 프레임워크의 내장 참조 벡터/문서 저장소를 그대로 대체하는 드롭인 어댑터. 공개 표면·영속 의미·리트리버·파이프라인 배선이 같으므로, import만 바꾸면 파이프라인을 유지한 채 교체된다.

| 프레임워크 | 설치                                  | 대체 대상                                                    |
| ---------- | ------------------------------------- | ------------------------------------------------------------ |
| LangChain  | `pip install turbovec[langchain]`   | `langchain_core.vectorstores.InMemoryVectorStore`          |
| LlamaIndex | `pip install turbovec[llama-index]` | `llama_index.core.vector_stores.SimpleVectorStore`         |
| Haystack   | `pip install turbovec[haystack]`    | `haystack.document_stores.in_memory.InMemoryDocumentStore` |
| Agno       | `pip install turbovec[agno]`        | `agno.vectordb.lancedb.LanceDb`                            |

---

## 빌드

### Python (maturin 사용)

```bash
pip install maturin
cd turbovec-python
maturin build --release
pip install target/wheels/*.whl
```

### Rust

```bash
cargo build --release
```

모든 x86_64 빌드는 `.cargo/config.toml`을 통해 `x86-64-v3`(AVX2 기준선, Haswell 2013년 이후)을 타깃으로 한다. AVX2 폴백 커널을 돌릴 수 있는 CPU면 전체 크레이트가 동작하며, AVX-512 커널은 런타임에 `is_x86_feature_detected!`로 감지해 지원 하드웨어에서만 켜진다.

---

## 벤치마크 실행

데이터셋 다운로드:

```bash
python3 benchmarks/download_data.py all            # 전체
python3 benchmarks/download_data.py glove          # GloVe 200차원
python3 benchmarks/download_data.py openai-1536    # OpenAI DBpedia 1536차원
python3 benchmarks/download_data.py openai-3072    # OpenAI DBpedia 3072차원
```

각 벤치마크는 `benchmarks/suite/`의 자기완결 스크립트다. 개별 실행:

```bash
python3 benchmarks/suite/speed_d1536_2bit_arm_mt.py
python3 benchmarks/suite/recall_d1536_2bit.py
python3 benchmarks/suite/compression.py
```

카테고리별 일괄 실행:

```bash
for f in benchmarks/suite/speed_*arm*.py; do python3 "$f"; done    # ARM 속도 전체
for f in benchmarks/suite/speed_*x86*.py; do python3 "$f"; done    # x86 속도 전체
for f in benchmarks/suite/recall_*.py; do python3 "$f"; done       # 회복률 전체
python3 benchmarks/suite/compression.py                            # 압축
```

결과는 `benchmarks/results/`에 JSON으로 저장된다. 차트 재생성:

```bash
python3 benchmarks/create_diagrams.py
```

---

## 인코딩 비용과 성능 특성

- 인코딩 비용: 벡터당 `⟨u, x̂⟩`를 계산하기 위한 d차원 내적 한 번이 추가된다. 100만 벡터 1,536차원에서 추가 인코딩 시간은 1초 미만으로, 쿼리가 아니라 인제스트 시점에 한 번 치르는 비용이다.
- 검색 시점: 데이터베이스 벡터를 복원하지 않는다. 쿼리를 같은 회전 행렬로 처리한 뒤 코드북 중심값과 직접 점수화한다.
- SIMD 커널: ARM은 NEON, x86은 AVX-512BW(또는 AVX2 폴백), 니블 분할 룩업 테이블과 u16 누산기 전략을 쓴다.

---

## 한계와 주의사항

- 저차원 벡터: GloVe 200차원처럼 차원이 낮으면 Beta 분포의 점근적 근사가 약해져 2비트 성능이 다소 떨어질 수 있다. TQ+ 캘리브레이션이 이 격차를 줄이지만 완전히 없애지는 못한다.
- 학습 불필요의 트레이드오프: 고정된 양자화 버킷을 쓰므로 극도로 편향된 임베딩 분포에서는 성능이 저하될 수 있다.
- x86 멀티 스레드: AVX-512 VBMI 경로에서 특정 케이스(2비트, 1536·3072차원)는 FAISS 대비 2-4% 뒤질 수 있다.
- 양자화는 손실 압축이다: 4비트·2비트 모두 근사 최근접 이웃(approximate nearest neighbor, ANN) 검색이라 정확 검색 대비 회복률 손실이 있다. 압축·속도 이득은 코퍼스가 수십만 건 이상으로 클 때 의미가 크고, 수천 건 규모에서는 정확 검색 대비 실익이 작다.

---

## 라이선스

MIT License. Copyright (c) 2026 Ryan Codrai. 상업적 활용·수정·재배포 자유.

---

## 참고 문헌

- TurboQuant: Online Vector Quantization with Near-optimal Distortion Rate (ICLR 2026) — 이 구현이 따르는 논문. arxiv.org/abs/2504.19874
- RaBitQ: Quantizing High-Dimensional Vectors with a Theoretical Error Bound for Approximate Nearest Neighbor Search (SIGMOD 2024) — 5단계 벡터별 길이 재정규화 보정의 출처. arxiv.org/abs/2405.12497
- FAISS Fast accumulation of PQ and AQ codes (FastScan) — turbovec의 x86 SIMD 커널이 채택한 패킹 레이아웃·니블 LUT 스코어링·u16 누산기 전략의 출처.
