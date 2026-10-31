//! Tiny UI translation layer: English strings are the keys, Korean is the only extra table.

use std::sync::atomic::{AtomicBool, Ordering};

static KOREAN: AtomicBool = AtomicBool::new(false);

pub fn set_korean(on: bool) {
    KOREAN.store(on, Ordering::Relaxed);
}

/// True if the OS UI language is Korean.
pub fn system_is_korean() -> bool {
    sys_locale::get_locale().is_some_and(|l| l.to_lowercase().starts_with("ko"))
}

/// Translate a UI string. Unknown strings (and English mode) are returned unchanged.
pub fn tr(s: &'static str) -> &'static str {
    if !KOREAN.load(Ordering::Relaxed) {
        return s;
    }
    match s {
        // menus
        "File" => "파일",
        "Edit" => "편집",
        "View" => "보기",
        "Encoding" => "인코딩",
        "Syntax" => "문법",
        "Font" => "글꼴",
        "Git" => "Git",
        "Settings" => "설정",
        // file
        "New" => "새로 만들기",
        "Open..." => "열기...",
        "Save" => "저장",
        "Save As..." => "다른 이름으로 저장...",
        "Close Tab" => "탭 닫기",
        "Exit" => "끝내기",
        "Untitled" => "제목 없음",
        "New tab (Ctrl+N)" => "새 탭 (Ctrl+N)",
        // view
        "Line Numbers" => "줄 번호",
        "Word Wrap" => "자동 줄 바꿈",
        "Wrap" => "줄 바꿈",
        "Theme" => "테마",
        "System" => "시스템",
        "Light" => "밝음",
        "Dark" => "어두움",
        "Syntax theme (dark mode)" => "문법 강조 테마 (어두운 모드)",
        "Syntax theme (light mode)" => "문법 강조 테마 (밝은 모드)",
        // edit / find
        "Cut" => "잘라내기",
        "Copy" => "복사",
        "Paste" => "붙여넣기",
        "Select All" => "모두 선택",
        "Zoom In" => "확대",
        "Zoom Out" => "축소",
        "Reset Zoom" => "확대/축소 초기화",
        "Settings..." => "설정...",
        "Add Next Occurrence" => "다음 항목을 선택에 추가",
        "Skip Occurrence" => "이 항목 건너뛰고 다음 선택",
        "Select All Occurrences" => "모든 항목 선택",
        "Add Cursors to Line Ends" => "줄 끝마다 커서 추가",
        "Updates" => "업데이트",
        "Check for updates automatically" => "업데이트 자동 확인",
        "Check for updates" => "업데이트 확인",
        "Checking for updates..." => "업데이트를 확인하는 중...",
        "You are up to date" => "최신 버전입니다",
        "Update installed, restart to apply:" => "업데이트가 설치됨, 다시 시작하면 적용:",
        "Update check failed" => "업데이트 확인 실패",
        "Updates are disabled in this build" => "이 빌드(개발용)에서는 업데이트가 꺼져 있습니다",
        "The update was installed. Restart now to use it?" => "업데이트가 설치되었습니다. 지금 다시 시작해서 적용할까요?",
        "Restart now" => "지금 다시 시작",
        "Later" => "나중에",
        "Open Path..." => "경로로 열기...",
        "Type a path (absolute or relative)..." => "경로 입력 (절대 경로 또는 상대 경로)...",
        "Folder:" => "폴더:",
        "No such folder" => "없는 폴더입니다",
        "New file:" => "새 파일:",
        "No matching path" => "일치하는 경로가 없습니다",
        "Undo" => "실행 취소",
        "Redo" => "다시 실행",
        "Find..." => "찾기...",
        "Replace..." => "바꾸기...",
        "Find Next" => "다음 찾기",
        "Find Previous" => "이전 찾기",
        "Find" => "찾기",
        "Replace" => "바꾸기",
        "Replace All" => "모두 바꾸기",
        "Previous (Shift+F3)" => "이전 (Shift+F3)",
        "Next (F3)" => "다음 (F3)",
        "Match case" => "대소문자 구분",
        "Whole word" => "단어 단위",
        "Regular expression" => "정규식",
        "invalid regex" => "잘못된 정규식",
        "No results" => "결과 없음",
        "matches" => "개 일치",
        "Replaced occurrences:" => "바꾼 항목 수:",
        // encoding
        "Current:" => "현재:",
        "Reopen with Encoding" => "인코딩을 지정하여 다시 열기",
        "Save with Encoding" => "인코딩을 지정하여 저장",
        "Write BOM on save" => "저장 시 BOM 기록",
        "Remove BOM and Save" => "BOM 제거 후 저장",
        "LF (Unix)" => "LF (유닉스)",
        "CRLF (Windows)" => "CRLF (윈도우)",
        "Save the file first to reopen it with another encoding" => {
            "다른 인코딩으로 다시 열려면 먼저 파일을 저장하세요"
        }
        "Reopened as" => "다시 열기:",
        "Failed to reopen" => "다시 열기 실패",
        "Some characters cannot be represented in" => {
            "다음 인코딩으로 표현할 수 없는 문자가 있습니다:"
        }
        "; file not saved" => " — 저장하지 않았습니다",
        // syntax / font
        "Filter..." => "필터...",
        "Auto-detect" => "자동 감지",
        "Size" => "크기",
        "Family:" => "글꼴:",
        "Filter fonts..." => "글꼴 검색...",
        // dialogs / status
        "Unsaved changes" => "저장되지 않은 변경 사항",
        "Discard unsaved changes to:" => "다음 파일의 변경 사항을 버릴까요:",
        "Discard" => "버리기",
        "Cancel" => "취소",
        "Saved" => "저장됨",
        "Failed to save" => "저장 실패",
        "Failed to open" => "열기 실패",
        "chars" => "자",
        "Ln" => "줄",
        "Col" => "열",
        "Subject" => "제목",
        "Line ending" => "줄 끝 형식",
        // settings
        "Highlighting engine" => "하이라이트 엔진",
        "Use tree-sitter highlighting when available" => "가능하면 tree-sitter 하이라이트 사용",
        "tree-sitter plugin loaded" => "tree-sitter 플러그인 로드됨",
        "languages" => "개 언어",
        "tree-sitter plugin not found" => "tree-sitter 플러그인을 찾을 수 없음",
        "tree-sitter plugin failed to load" => "tree-sitter 플러그인 로드 실패",
        "Place nanote_tree_sitter.dll next to nanote.exe." => {
            "nanote_tree_sitter.dll을 nanote.exe와 같은 폴더에 두세요."
        }
        "General" => "일반",
        "Appearance" => "모양",
        "Restore previously opened files on startup" => "시작 시 이전에 열었던 파일 복원",
        "Use nanote as the editor for git commit messages, rebase todo lists, etc." => {
            "git 커밋 메시지, 리베이스 목록 등을 편집할 때 nanote를 사용합니다."
        }
        "Interface language" => "표시 언어",
        "Open command-line files in a new window" => "명령줄로 지정한 파일을 새 창에서 열기",
        // git
        "Set as Git editor" => "Git 편집기로 설정",
        "Unset Git editor" => "Git 편집기 설정 해제",
        "Show current Git editor" => "현재 Git 편집기 확인",
        "Git editor set to" => "Git 편집기로 설정됨:",
        "Git editor unset" => "Git 편집기 설정 해제됨",
        "Git editor is" => "현재 Git 편집기:",
        "Git editor is not set" => "Git 편집기가 설정되어 있지 않음",
        "Git command failed" => "Git 명령 실패",
        _ => s,
    }
}
