use core::ops::{Deref, DerefMut};

use crate::driver::mcp23009::{Direction, Mcp23009, OutputState};

pub struct IoExpander {
    pub(crate) driver: Mcp23009<'static>,
}

pub type Error = crate::driver::mcp23009::Error;

impl Deref for IoExpander {
    type Target = Mcp23009<'static>;

    fn deref(&self) -> &Self::Target {
        &self.driver
    }
}
impl DerefMut for IoExpander {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.driver
    }
}

impl IoExpander {
    pub fn init(driver: Mcp23009<'static>) -> Result<Self, Error> {
        let mut ioexpander = Self { driver };
        ioexpander.configure()?;

        Ok(ioexpander)
    }

    pub fn configure(&mut self) -> Result<(), Error> {
        use Direction::{Input, Output};

        self.driver.init()?;
        self.driver.set_outputs([OutputState::Released; 8])?;
        self.driver
            .set_directions([Input, Input, Output, Output, Output, Output, Output, Input])?;

        Ok(())
    }
}
