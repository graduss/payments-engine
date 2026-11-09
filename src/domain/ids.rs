use std::fmt::Display;

/// Client identifier (`u16` in the input).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClientId(u16);
impl From<u16> for ClientId {
    fn from(value: u16) -> Self {
        Self(value)
    }
}

impl Display for ClientId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Globally unique transaction identifier (`u32` in the input).
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct TxId(u32);
impl From<u32> for TxId {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

impl Display for TxId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_display_as_plain_numbers() {
        assert_eq!(ClientId::from(65535).to_string(), "65535");
        assert_eq!(TxId::from(4294967295).to_string(), "4294967295");
    }
}
