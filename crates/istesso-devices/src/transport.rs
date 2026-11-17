//! The raw device, as the framework sees it. An adapter never gets this: it
//! gets a [`crate::guard::Guard`] around it.

/// Reads and writes HID reports of one device. Errors are text for a log.
pub trait Transport {
    /// Send an output report. `report[0]` is the report ID.
    fn write(&mut self, report: &[u8]) -> Result<(), String>;
    /// Send a feature report. `report[0]` is the report ID.
    fn send_feature(&mut self, report: &[u8]) -> Result<(), String>;
    /// Ask for a feature report. Fills `buffer` from the report ID on and
    /// returns the number of bytes.
    fn get_feature(&mut self, id: u8, buffer: &mut [u8]) -> Result<usize, String>;
    /// Wait up to `timeout_ms` for an input report. 0 bytes means that
    /// nothing came.
    fn read(&mut self, buffer: &mut [u8], timeout_ms: u32) -> Result<usize, String>;
}

/// A device that is not there: for tests and for replaying recordings.
///
/// It remembers what was sent and answers feature requests from a queue.
#[derive(Debug, Default)]
pub struct FakeTransport {
    /// Answers to `get_feature`, in the order they will be asked for.
    pub answers: std::collections::VecDeque<(u8, Vec<u8>)>,
    /// Input reports the device "sends", in order.
    pub inputs: std::collections::VecDeque<Vec<u8>>,
    /// Output reports that were written.
    pub written: Vec<Vec<u8>>,
    /// Feature reports that were sent.
    pub features: Vec<Vec<u8>>,
}

impl Transport for FakeTransport {
    fn write(&mut self, report: &[u8]) -> Result<(), String> {
        self.written.push(report.to_vec());
        Ok(())
    }

    fn send_feature(&mut self, report: &[u8]) -> Result<(), String> {
        self.features.push(report.to_vec());
        Ok(())
    }

    fn get_feature(&mut self, id: u8, buffer: &mut [u8]) -> Result<usize, String> {
        match self.answers.pop_front() {
            Some((answer_id, bytes)) if answer_id == id => {
                let n = bytes.len().min(buffer.len());
                buffer[..n].copy_from_slice(&bytes[..n]);
                Ok(n)
            }
            Some((answer_id, _)) => Err(format!(
                "feature report {id:#04x} was asked for, the next recorded answer is for {answer_id:#04x}"
            )),
            None => Err(format!("feature report {id:#04x} was asked for, no answer is left")),
        }
    }

    fn read(&mut self, buffer: &mut [u8], _timeout_ms: u32) -> Result<usize, String> {
        match self.inputs.pop_front() {
            Some(bytes) => {
                let n = bytes.len().min(buffer.len());
                buffer[..n].copy_from_slice(&bytes[..n]);
                Ok(n)
            }
            None => Ok(0),
        }
    }
}
