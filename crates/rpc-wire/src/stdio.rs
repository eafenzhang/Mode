//! stdio 分帧：`[4 字节大端长度][payload]`。
//!
//! 与 `packages/desktop/src/host-standalone/stdioFrames.ts` 同语义：单帧上限 64 MiB，
//! 超限即判定流不可恢复（后续字节全部丢弃，只报一次）；粘包/分片由解码器自己处理。

pub const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StdioError {
    FrameTooLarge { length: u32, max: usize },
}

#[derive(Debug, Default)]
pub struct StdioFrameDecoder {
    pending: Vec<u8>,
    broken: bool,
}

impl StdioFrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// 流已判定不可恢复：调用方应停止读取并记录一次诊断。
    pub fn is_broken(&self) -> bool {
        self.broken
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// 投喂一个 chunk，返回本次凑齐的完整帧。
    ///
    /// 超限时返回 `Err(FrameTooLarge)` 且此后恒返回空——复制 TS 的「只报一次」语义。
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<Vec<u8>>, StdioError> {
        if self.broken {
            return Ok(Vec::new());
        }
        self.pending.extend_from_slice(chunk);
        let mut frames = Vec::new();
        loop {
            if self.pending.len() < 4 {
                return Ok(frames);
            }
            let length = u32::from_be_bytes([
                self.pending[0],
                self.pending[1],
                self.pending[2],
                self.pending[3],
            ]);
            if length as usize > MAX_FRAME_BYTES {
                self.broken = true;
                self.pending.clear();
                return Err(StdioError::FrameTooLarge {
                    length,
                    max: MAX_FRAME_BYTES,
                });
            }
            let total = 4 + length as usize;
            if self.pending.len() < total {
                return Ok(frames);
            }
            frames.push(self.pending[4..total].to_vec());
            self.pending.drain(..total);
        }
    }
}

/// 编码一帧（4 字节大端长度 + payload）。
pub fn encode_stdio_frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_frames_split_across_chunks() {
        let frame = encode_stdio_frame(b"hello");
        let mut decoder = StdioFrameDecoder::new();
        let mut collected = Vec::new();
        for byte in &frame {
            collected.extend(decoder.push(std::slice::from_ref(byte)).unwrap());
        }
        assert_eq!(collected, vec![b"hello".to_vec()]);
        assert_eq!(decoder.pending_len(), 0);
    }

    #[test]
    fn decodes_multiple_frames_in_one_chunk() {
        let mut bytes = encode_stdio_frame(b"a");
        bytes.extend(encode_stdio_frame(b"bb"));
        let mut decoder = StdioFrameDecoder::new();
        assert_eq!(
            decoder.push(&bytes).unwrap(),
            vec![b"a".to_vec(), b"bb".to_vec()]
        );
    }

    #[test]
    fn rejects_oversized_length_once_then_stays_broken() {
        let mut bytes = ((MAX_FRAME_BYTES as u32) + 1).to_be_bytes().to_vec();
        bytes.extend_from_slice(b"payload");
        let mut decoder = StdioFrameDecoder::new();
        assert!(matches!(
            decoder.push(&bytes),
            Err(StdioError::FrameTooLarge { .. })
        ));
        assert!(decoder.is_broken());
        assert!(decoder.push(b"more").unwrap().is_empty());
    }

    #[test]
    fn waits_for_complete_header() {
        let mut decoder = StdioFrameDecoder::new();
        assert!(decoder.push(&[0, 0]).unwrap().is_empty());
        assert_eq!(decoder.pending_len(), 2);
    }
}
