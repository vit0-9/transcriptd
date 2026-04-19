use transcriptd_claude::ClaudeExtractor;
use transcriptd_codex::CodexExtractor;
use transcriptd_core::TranscriptExtractor;
use transcriptd_cursor::CursorExtractor;
use transcriptd_vscode::VscodeExtractor;
use transcriptd_zed::ZedExtractor;

pub fn all_extractors() -> Vec<Box<dyn TranscriptExtractor>> {
    vec![
        Box::new(ZedExtractor),
        Box::new(ClaudeExtractor),
        Box::new(VscodeExtractor),
        Box::new(CodexExtractor),
        Box::new(CursorExtractor),
    ]
}
