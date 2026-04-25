/// Domain model representing a swap with its identifier and amount.
#[derive(Debug, Clone)]
pub struct Swap {
    pub swap_id: String,
    pub amount: i64,
}

/// A vector that is guaranteed to contain at least one element.
#[derive(Debug, Clone)]
pub struct Vec1<T>(Vec<T>);

impl<T> Vec1<T> {
    pub fn new(vec: Vec<T>) -> Result<Self, eyre::Report> {
        if vec.is_empty() {
            eyre::bail!("Cannot create NonEmptyVec from empty vector");
        }
        Ok(Self(vec))
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[allow(dead_code)]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false // guaranteed non-empty by construction
    }
}

impl<T> AsRef<[T]> for Vec1<T> {
    fn as_ref(&self) -> &[T] {
        &self.0
    }
}

impl<T> TryFrom<Vec<T>> for Vec1<T> {
    type Error = eyre::Report;

    fn try_from(vec: Vec<T>) -> Result<Self, Self::Error> {
        Self::new(vec)
    }
}

impl<T> std::ops::Deref for Vec1<T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
