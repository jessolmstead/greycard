# Korean (ko) glossary for greycard's window

This is the glossary of a draft translation that no native speaker has
read yet. Corrections to it and to the catalog are welcome.

Source of the terms: the Korean Lightroom Classic (LrC) first; Capture
One (C1), the Korean menus of Canon, Sony, Nikon, and Fujifilm bodies,
and the words Korean photographers say where LrC has none or its word
is not the one in use.

## Conventions

- **Register.** 합니다체 throughout, as Korean desktop software
  (Windows, Adobe) writes it: a sentence ends in 합니다, 했습니다,
  없습니다; a request in 하십시오 ("폴더를 선택하십시오"); a question
  in 합니까 or 하시겠습니까 ("사진 3장을 삭제하시겠습니까?"). 해요체 is
  never used. Buttons, menu items, and labels are nouns or noun phrases
  ending in a noun ("내보내기", "폴더 열기...", "모두 재설정"), never a
  verb ending. A status line that reports what happened is a full
  sentence in the past ("사진 3장을 가져왔습니다"), except where code
  joins pieces after it (the import's and the delete's reports), which
  end in a noun ("사진 3장 가져오기 완료, 2장 백업"); one still running
  is a noun phrase in 중 ("내보내는 중...", "읽는 중..."). A reason
  fragment that fills an error ("reading it: {e}") is a noun phrase
  ("읽기 실패: {e}" or "읽을 때: {e}").
- **Particles after a placeholder.** Korean particles change with the
  word before them (을/를, 이/가, 은/는, 와/과, 으로/로), and a
  placeholder's last sound is not known. Where a noun can follow the
  placeholder the sentence is built so the particle follows the noun
  ("{name} 룩을"); otherwise both forms are written as Korean Windows
  writes them: 을(를), 이(가), 은(는), 와(과), (으)로.
- **Punctuation.** Half-width throughout, as modern Korean writes it:
  . , ? ! : ( ). A name in a sentence that the English quotes takes
  single quotes ('{name}'). Menus and running states keep the
  source's "..." with no space before it. A status line ends without a
  period where the English has none. Two clauses the English joins
  with "; " become two sentences joined with ". " when the first ends
  in 니다, as Korean rarely writes a semicolon inside a sentence.
- **Spaces.** Korean word spacing (띄어쓰기). A number and its counter
  are written together ("{n}장", "3개"); a number and a unit symbol
  keep a space ("0.5 MB", "{size} KB"). A particle joins the word or
  placeholder before it ("{folder}에"). The " · " and " — "
  separators and a key before its word ("P 채택 · X 제외") are kept.
- **Headings.** The develop panel's ALL CAPS headings are plain Korean,
  which has no capitals ("WHITE BALANCE" → 화이트 밸런스).
- **Counters.** 장 for frames, pictures, and photos ("사진 {n}장");
  개 for things (files, sidecars, folders, groups, presets, looks,
  masks, keywords); 건 for records (edits, failures, history steps,
  exports); 회 for repeated actions. One plural form
  (nplurals=1; plural=0); where English's singular and plural differ in
  more than the count, the form that reads right for both was taken.
- **Placeholders.** Reordered as Korean needs (the verb last); a `{}`
  that moves is written `{0}`, `{1}`.
- **Joined fragments.** A tail the English joins with ", " starts with
  ", "; one joined with "; " starts with "; ", as Korean writes both
  half-width. A few joins are built in code with an English "; " or
  ", " the catalog cannot reach (the delete's refusal, the archive
  copy's report, the sync note, lists of names): those read the same.
- **Product words.** greycard, Lightroom, RawTherapee, darktable,
  lensfun, GitHub, ONNX Runtime, JPEG, TIFF, PNG, DNG, ICC, EXIF, XMP,
  RGB, HDR, GPU, CPU, AgX, file names, units (MB, KB, GiB, px, EV, K,
  mired), and the search's tokens (`camera:`, `iso>=`) stay as they
  are. RAW is written in capitals, as Korean camera makers and
  photographers write it. Seconds are 초 ("{seconds}초").
- **Keys.** Key names stay as the keys sheet draws them (Ctrl, Shift,
  Alt, Space, Enter, Esc, Tab), as docs/translating.md asks. The
  pointer's word is translated ("Ctrl+클릭", "오른쪽 클릭"). Key
  ranges use a tilde ("1~5, 0").
- **Numbers and dates** keep the English form (§616).
- **Frame / picture.** A frame (a file in the library) is 사진, counted
  장; a picture (what is drawn, the image itself) is 이미지.
- **Develop.** The mode is 현상 (LrC's module); the verb, a raw being
  rendered for the screen or an export, is 현상하다 ("현상 중...").
- **Loanwords** follow the National Institute of Korean Language's
  spelling, as Adobe Korea does: 스냅숏, 레이블, 그레이디언트,
  메시지, 컬렉션.

## Terms

| English | Korean | Why / alternative |
| --- | --- | --- |
| Library | 라이브러리 | LrC module name |
| Develop | 현상 | LrC module name |
| Loupe | 확대경 | LrC "확대경 보기" |
| Lightbox | 라이트 박스 | standard |
| Filmstrip | 필름 스트립 | LrC |
| Grid | 격자 | LrC "격자 보기" |
| Panel / pane | 패널 / 창 | LrC |
| Fit (zoom) | 맞춤 | LrC navigator |
| Exposure | 노출 | LrC |
| Contrast | 대비 | LrC |
| Highlights | 밝은 영역 | LrC, its clipping warning included ("밝은 영역 클리핑"); "하이라이트" is what photographers say, kept for Highlight fill alone |
| Shadows | 어두운 영역 | LrC |
| Midtones | 중간 톤 | LrC Color Grading |
| Whites / Blacks | 흰색 계열 / 검정 계열 | LrC |
| LIGHT (the section) | 조명 | Lightroom (mobile and the newer panels) "조명"; parametric Lights is 밝은 톤, in another section |
| Clarity | 부분 대비 | LrC; "클래리티" is heard but LrC's word is the one in every tutorial |
| Texture | 텍스처 | LrC |
| Dehaze | 디헤이즈 | LrC and ACR; alt. "안개 제거" |
| Vibrance | 생동감 | LrC |
| Saturation | 채도 | LrC |
| White balance | 화이트 밸런스 | every camera maker's menu; LrC's own "흰색 균형" is rarely said |
| Temperature (WB) | 색온도 | LrC, camera menus |
| Tint (WB) | 틴트 | C1 and what retouchers say ("색온도와 틴트"); LrC's "색조" is also its Hue, and both show in a mask's look here |
| As shot | 촬영 시 설정 | LrC "촬영 당시"; shortened |
| Auto | 자동 | |
| Daylight, Cloudy, Shade, Tungsten, Fluorescent, Flash | 일광, 흐림, 그늘, 텅스텐, 형광등, 플래시 | LrC and camera menus |
| Neutral (WB dropper) | 중립 | |
| TINT / Color tint (the section that lays a color over) | 색 입히기 | kept apart from WB's 틴트; alt. "컬러 틴트", "착색" |
| Tone curve / Curves | 톤 곡선 / 곡선 | LrC |
| Lights / Darks (parametric) | 밝은 톤 / 어두운 톤 | LrC |
| Point / Parametric | 점 / 매개변수 | LrC "점 곡선 / 매개변수 곡선" |
| Per channel | 채널별 | |
| Color mixer | 색상 혼합 | LrC |
| Color (a label, the noun) | 색상 | LrC "색상 레이블" |
| Hue / Luminance | 색조 / 광도 | LrC HSL |
| Red, Orange, Yellow, Green, Aqua, Blue, Purple, Magenta | 빨강, 주황, 노랑, 초록, 아쿠아, 파랑, 자주, 마젠타 | LrC color mixer and color labels |
| Color grading | 컬러 그레이딩 | what Korean photographers and video editors call it; LrC "색 보정" reads as any color correction |
| Balance (grading) | 균형 | LrC |
| Black & white | 흑백 | LrC |
| Weight (B&W filter) | 가중치 | none in LrC |
| Sharpen | 선명하게 하기 | LrC; 선명도 in a short label |
| Output sharpen | 출력 선명도 | LrC export "출력 선명하게 하기", shortened |
| Amount / Radius / Detail / Threshold | 양 / 반경 / 세부 / 임계값 | LrC |
| Detail (the section) | 세부 | LrC |
| Noise / Denoise | 노이즈 / 노이즈 제거 | LrC (AI "노이즈 제거") |
| Luma / Chroma | 광도 / 크로마 | LrC says 광도 / 색상 for noise; 크로마 (dt) because the same word is also the color range's chroma floor, where 색상 would mislead |
| Structure (Detail's widest band) | 구조 | Texture is already 텍스처 |
| Grain | 그레인 | LrC |
| Cubic / Tabular (grain) | 입방형 / 평판형 | film's cubic and T-grain |
| Vignette (the effect) | 비네팅 | LrC "자르기 후 비네팅" |
| Vignetting (the lens's) | 주변 광량 | Canon's "주변 광량 보정"; kept apart from the effect |
| Midpoint / Feather / Roundness | 중간점 / 페더 / 원형률 | LrC |
| Lens corrections | 렌즈 교정 | LrC |
| Chromatic aberration | 색수차 | LrC |
| Defringe | 언저리 제거 | LrC |
| Distortion | 왜곡 | LrC |
| Scale / Auto scale | 비율 / 자동 비율 | LrC Transform "비율" |
| Crop | 자르기 | LrC |
| Aspect | 종횡비 | LrC |
| Original (aspect) | 원본 | LrC |
| Rotate (part of the edit) | 회전 | LrC |
| Turn (a quarter, not part of the edit) | 돌리기 / 왼쪽으로 돌리기 | kept apart from 회전 |
| Level | 수평 맞추기 | camera menus' 수평; LrC's "똑바르게" tool is rarely named |
| Flip / Flip vertical | 가로 뒤집기 / 세로 뒤집기 | LrC "가로로 뒤집기"; plain 반전 is Invert |
| Perspective | 원근 보정 | LrC's panel is "변환" |
| Guides: Thirds, Golden ratio, Golden spiral, Diagonals, Triangles | 안내선: 삼등분, 황금 비율, 황금 나선, 대각선, 삼각형 | LrC crop overlays |
| Mask / Adjustment | 마스크 / 조정 | LrC |
| Shape | 모양 | LrC calls them "구성 요소"; alt. that |
| Global | 전체 | |
| Linear / Radial | 선형 / 방사형 | LrC "선형 그레이디언트 / 방사형 그레이디언트", shortened as the English is |
| Brush / Paint | 브러시 / 칠하기 | LrC |
| Feather / Flow / Density | 페더 / 흐름 / 밀도 | LrC |
| Erase | 지우기 | LrC |
| Invert / Intersect / Subtract / Add | 반전 / 교차 / 빼기 / 추가 | LrC |
| Refine edge | 가장자리 다듬기 | Photoshop |
| Subject / Sky / Background / People / Object / Landscape | 피사체 / 하늘 / 배경 / 사람 / 개체 / 풍경 | LrC |
| Color range / Luminance range | 색상 범위 / 광도 범위 | LrC |
| Retouch | 리터치 | standard |
| Heal / Clone / Fill | 복구 / 복제 / 채우기 | LrC removal tool |
| Remove (the retouch tool's work) | 제거 | LrC "제거" |
| Best / Fast / Patch (fill quality) | 최고 품질 / 빠르게 / 패치 | |
| Look | 룩 | photographers' word; LrC's "프로필" means the camera profile here |
| Picture style (a camera's) | 픽처 스타일 | Canon's Korean |
| Camera profile | 카메라 프로필 | LrC |
| Camera match / Fit (a look) | 카메라 매치 / 맞추기 | none in LrC |
| Display curve | 톤 매핑 | none in LrC; alt. "표시 곡선" |
| Highlight fill | 하이라이트 보완 | none in LrC; alt. "하이라이트 복원", dt "하이라이트 재구성" |
| Demosaic | 디모자이크 | dt |
| Preset | 프리셋 | what photographers say and sell ("라이트룸 프리셋"); LrC "사전 설정" is longer |
| Reset | 재설정 | LrC |
| Snapshot | 스냅숏 | LrC |
| History | 작업 내역 | LrC |
| Restore (a snapshot) / Bring back (from an archive) | 복원 | the same word for both; they never share a menu |
| Virtual copy / Version | 가상 사본 / 버전 | LrC |
| Edit (the noun) | 편집 | |
| Sidecar | 사이드카 | LrC "XMP 사이드카 파일", shortened |
| Cull / Culling | 선별 | alt. "컬링", "셀렉" (wedding photographers' word) |
| Rating / stars | 등급 / 별 | LrC; "별 {n}개" |
| Flag / Pick / Reject / Unflagged / Unflag | 플래그 / 채택 / 제외 / 플래그 없음 / 플래그 해제 | LrC says "플래그 지정됨" for Pick, which reads as any flag; 채택 is the cull word |
| Rejects (the frames, the folder) | 제외한 사진 / 제외 폴더 | |
| Label | 레이블 | LrC "색상 레이블", shortened |
| Keyword | 키워드 | LrC |
| Title | 제목 | LrC |
| Stack | 스택 | LrC |
| Collection / Smart collection / Collection set | 컬렉션 / 스마트 컬렉션 / 컬렉션 세트 | LrC |
| Root (a library root) | 루트 폴더 | alt. "소스" |
| Archive / Back up | 아카이브 / 백업 | |
| Card | 카드 | memory card |
| Export / Import | 내보내기 / 가져오기 | LrC |
| Add in place (import) | 추가 | LrC's import "추가" |
| From / To (import rows) | 원본 / 대상 | |
| Copy / Paste settings | 설정 복사 / 설정 붙여넣기 | LrC |
| Sync | 동기화 | LrC |
| Merge to HDR / Panorama / Focus stack | HDR 병합 / 파노라마 병합 / 초점 합성 | LrC "사진 병합"; 초점 합성 is the common Korean word |
| Soft proof | 소프트 교정 | LrC |
| Gamut warning | 색역 경고 | |
| Intent / Perceptual / Relative | 의도 / 지각 / 상대 | LrC soft proofing |
| Long edge | 긴 변 | LrC "긴 가장자리" is longer |
| If it exists / Increment / Overwrite / Skip | 이미 있으면 / 번호 붙이기 / 덮어쓰기 / 건너뛰기 | |
| Watermark | 워터마크 | LrC |
| Border | 테두리 | |
| Metadata | 메타데이터 | LrC |
| Capture time | 촬영 시간 | LrC |
| Hand order | 사용자 순서 | LrC |
| Navigator | 탐색기 | LrC |
| Canvas | 캔버스 | LrC "배경"; 배경 is the mask's Background |
| Scope / Selection (scopes) | 스코프 / 선택 영역 | |
| Clipping (warnings) | 클리핑 | LrC "클리핑" |
| Focus peaking | 피킹 | camera menus |
| Thumbnail / Preview | 축소판 / 미리 보기 | LrC |
| Filter | 필터 | |
| Offline / isn't reachable | 오프라인 / 연결할 수 없음 | |
| Trash | 휴지통 | Windows; macOS also "휴지통" |
| Favorites | 즐겨찾기 | |
| Remove / Delete | 제거 / 삭제 | 제거 takes something out (a look, a shape, from a collection); 삭제 destroys a file or a version |
| Check / Uncheck (a row's box) | 선택 / 선택 해제 | Windows' word for a check box |
| Pick (the color mixer's dropper) | 스포이드 | Photoshop |
| Set / Shift (the sync sheet's two ways) | 지정 / 증감 | 증감 because the value is added to each frame's own; 이동 is the time shift |
| Shift capture time / time shift | 촬영 시간 이동 / 시간 이동 | |
| Export queue / queue | 내보내기 대기열 / 대기열 | |
| Library index / indexing | 라이브러리 색인 / 색인 | |
| Forget (missing frames) | 등록 해제 | |
| Stack bursts | 연사 스택 | camera menus' 연사 |
| Sort orders | 최신순, 오래된순, 높은순, 낮은순, A~Z, Z~A | |
| Body (a camera's) | 바디 | what photographers say |
| Directory | 디렉터리 | the standard spelling |
| Fetch / Download (a model) | 받기 / 다운로드 | |
| Settings / About | 설정 / 정보 | |
| Interface (Settings' category) | 인터페이스 | |
| Keys (the sheet) | 단축키 | |
| Undo / Redo | 실행 취소 / 다시 실행 | Windows |
| Model (AI) | 모델 | |
| Log | 로그 | |
