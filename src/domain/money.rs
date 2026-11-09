use super::error::DomainError;
use rust_decimal::Decimal;

/// Amount of a deposit or withdrawal: above zero, at most 4 decimal places.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Amount(Decimal);

impl Amount {
    pub const MAX_SCALE: u32 = 4;
}

/// Trailing zeros are dropped first, so `1.50000` passes. Nothing is rounded:
/// a value with too many places is rejected.
impl TryFrom<Decimal> for Amount {
    type Error = DomainError;
    fn try_from(value: Decimal) -> Result<Self, Self::Error> {
        let value = value.normalize();
        if value <= Decimal::ZERO {
            return Err(DomainError::AmountNotPositive);
        }
        if value.scale() > Self::MAX_SCALE {
            return Err(DomainError::AmountTooPrecise(Self::MAX_SCALE));
        }

        Ok(Self(value))
    }
}

impl From<Amount> for Decimal {
    fn from(value: Amount) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn accepts_positive_amount_with_up_to_four_places() {
        assert_eq!(Decimal::from(Amount::try_from(dec!(1)).unwrap()), dec!(1));
        assert_eq!(
            Decimal::from(Amount::try_from(dec!(0.0001)).unwrap()),
            dec!(0.0001)
        );
        assert_eq!(
            Decimal::from(Amount::try_from(dec!(12.3456)).unwrap()),
            dec!(12.3456)
        );
    }

    #[test]
    fn trailing_zeros_do_not_count_as_precision() {
        let amount = Amount::try_from(dec!(1.50000)).unwrap();
        assert_eq!(Decimal::from(amount), dec!(1.5));
        assert_eq!(Decimal::from(amount).scale(), 1);
    }

    #[test]
    fn rejects_more_than_four_places_without_rounding() {
        assert_eq!(
            Amount::try_from(dec!(1.00005)),
            Err(DomainError::AmountTooPrecise(Amount::MAX_SCALE))
        );
        assert_eq!(
            Amount::try_from(dec!(0.00001)),
            Err(DomainError::AmountTooPrecise(Amount::MAX_SCALE))
        );
    }

    #[test]
    fn rejects_zero_and_negative_amounts() {
        assert_eq!(
            Amount::try_from(dec!(0)),
            Err(DomainError::AmountNotPositive)
        );
        assert_eq!(
            Amount::try_from(dec!(0.0000)),
            Err(DomainError::AmountNotPositive)
        );
        assert_eq!(
            Amount::try_from(dec!(-1)),
            Err(DomainError::AmountNotPositive)
        );
        assert_eq!(
            Amount::try_from(dec!(-0.00001)),
            Err(DomainError::AmountNotPositive)
        );
    }
}
