//! Money type: integer minor units. Never floats.
//! `MoneyCents` is deliberately simple; multi-currency rules live in domain packs.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct MoneyCents {
    pub amount: i64,
    pub currency: [u8; 3],
}

impl MoneyCents {
    pub fn new(amount: i64, currency: &str) -> Result<Self, crate::AppError> {
        let bytes = currency.as_bytes();
        if bytes.len() != 3 || !bytes.iter().all(|b| b.is_ascii_uppercase()) {
            return Err(crate::AppError::validation(format!(
                "invalid ISO currency {currency:?}"
            )));
        }
        Ok(Self {
            amount,
            currency: [bytes[0], bytes[1], bytes[2]],
        })
    }

    pub fn currency_str(&self) -> &str {
        std::str::from_utf8(&self.currency).unwrap_or("???")
    }
}
