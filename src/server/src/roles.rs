//! Roles and permissions system.

bitflags::bitflags! {
    #[derive(Clone, Debug, Default)]
    pub struct RolePermissions: u32 {
        const NONE      = 0;
        const VIEW       = 1 << 0;
        const SEND       = 1 << 1;
        const KICK       = 1 << 2;
        const BAN        = 1 << 3;
        const MUTE       = 1 << 4;
        const MANAGE     = 1 << 5;
        const PIN        = 1 << 6;
    }
}

impl RolePermissions {
    pub fn from_db_row(
        can_kick: bool,
        can_ban: bool,
        can_mute: bool,
        can_manage: bool,
        can_pin: bool,
    ) -> Self {
        let mut perms = Self::NONE;
        perms |= Self::VIEW | Self::SEND;
        if can_kick {
            perms |= Self::KICK;
        }
        if can_ban {
            perms |= Self::BAN;
        }
        if can_mute {
            perms |= Self::MUTE;
        }
        if can_manage {
            perms |= Self::MANAGE;
        }
        if can_pin {
            perms |= Self::PIN;
        }
        perms
    }
}

pub struct Role {
    pub id: i64,
    pub name: String,
    pub level: i32,
    pub permissions: RolePermissions,
    pub created_at: f64,
}
