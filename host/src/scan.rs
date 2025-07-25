//! Scan config.
use bt_hci::cmd::le::{
    LeAddDeviceToFilterAcceptList, LeClearFilterAcceptList, LeSetExtScanEnable, LeSetExtScanParams, LeSetScanEnable,
    LeSetScanParams, LePeriodicAdvCreateSync, LePeriodicAdvCreateSyncParams, LeClearPeriodicAdvList, LeAddDeviceToPeriodicAdvList,
    //NEW ADDED
    // LE Set Periodic Advertising Receive Enable command [📖](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-54/out/en/host-controller-interface/host-controller-interface-functional-specification.html#UUID-b055b724-7607-bf63-3862-3e164dfc2251)
    //and param
    LeSetPeriodicAdvReceiveEnable,LeSetPeriodicAdvReceiveEnableParams

};
use bt_hci::controller::{Controller, ControllerCmdSync, ControllerCmdAsync};
use bt_hci::param::{AddrKind, FilterDuplicates, ScanningPhy};
pub use bt_hci::param::{LeAdvReportsIter, LeExtAdvReportsIter};
use embassy_time::Instant;

use crate::command::CommandState;
use crate::connection::ScanConfig;
use crate::{BleHostError, Central, PacketPool};

/// A scanner that wraps a central to provide additional functionality
/// around BLE scanning.
///
/// The buffer size can be tuned if in a noisy environment that
/// returns a lot of results.
pub struct Scanner<'d, C: Controller, P: PacketPool> {
    central: Central<'d, C, P>,
}

impl<'d, C: Controller, P: PacketPool> Scanner<'d, C, P> {
    /// Create a new scanner with the provided central.
    pub fn new(central: Central<'d, C, P>) -> Self {
        Self { central }
    }

    /// Retrieve the underlying central
    pub fn into_inner(self) -> Central<'d, C, P> {
        self.central
    }

    /// Enable extended scanning and synchronize with a periodic advertising train from an advertiser.
    /// 
    /// Details on the underlying command can be found here:
    /// https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-54/out/en/host-controller-interface/host-controller-interface-functional-specification.html#UUID-29188ef0-bf80-7807-2c96-385e7d9782ed
    pub async fn scan_periodic(&mut self, config: &ScanConfig<'_>, param: LePeriodicAdvCreateSyncParams) -> Result<ScanSession<'_, true>, BleHostError<C::Error>>
    where
        C: ControllerCmdAsync<LePeriodicAdvCreateSync> //Note the Async variation here
            + ControllerCmdSync<LeClearPeriodicAdvList>
            + ControllerCmdSync<LeAddDeviceToPeriodicAdvList>
            + ControllerCmdSync<LeSetPeriodicAdvReceiveEnable>
            + ControllerCmdSync<LeSetExtScanEnable> //We also enable extended scanning
            + ControllerCmdSync<LeSetExtScanParams>
            + ControllerCmdSync<LeClearFilterAcceptList>
            + ControllerCmdSync<LeAddDeviceToFilterAcceptList>,
    {
        //First we enable extended scanning.
        let host = &self.central.stack.host;
        let drop = crate::host::OnDrop::new(|| {
            host.scan_command_state.cancel(false);
        });
        
        host.scan_command_state.request().await;
        //self.central.set_accept_filter(config.filter_accept_list).await?; --maybe this causes an error

        let scanning = ScanningPhy {
            active_scan: config.active, 
            scan_interval: config.interval.into(),
            scan_window: config.window.into(),
        };
        
        let phy_params = crate::central::create_phy_params(scanning, config.phys);
        let host = &self.central.stack.host;
        
        //maybe comment this out to fall back on Vendor default
        host.command(LeSetExtScanParams::new(
            host.address.map(|s| s.kind).unwrap_or(AddrKind::PUBLIC),
            bt_hci::param::ScanningFilterPolicy::BasicUnfiltered, 
            phy_params,
        ))
        .await?;

        //This is the difference from simply scan_ext --it is fine to put this before ext scan enable
        // host.async_command(LePeriodicAdvCreateSync::new(
        //     param.options,
        //     param.adv_sid,
        //     param.adv_addr_kind,
        //     param.adv_addr,
        //     param.skip,
        //     param.sync_timeout,
        //     param.sync_cte_kind
        // ))
        // .await?;


        //config.window is the duration of the scan.
        //config.interval is the period of the scan.
        //We can scan continuously by settting these to 0 seconds.
        host.command(LeSetExtScanEnable::new(
            true,
            FilterDuplicates::Disabled,
            config.window,
            config.interval,
        ))
        .await?;

        
        //This is the difference from simply scan_ext
        host.async_command(LePeriodicAdvCreateSync::new(
            param.options,
            param.adv_sid,
            param.adv_addr_kind,
            param.adv_addr,
            param.skip,
            param.sync_timeout,
            param.sync_cte_kind
        ))
        .await?;

        // embassy_time::Timer::after_secs(5).await;
        // //This is indeed a sync command-- HACK: we know the SyncHandle will be 0
        // host.command(LeSetPeriodicAdvReceiveEnable::new(bt_hci::param::SyncHandle::default(), 
        // bt_hci::param::LePeriodicAdvReceiveEnable::new().set_duplicate_filtering(false).set_reporting(true))).await?;

        drop.defuse();
        Ok(ScanSession {
            command_state: &self.central.stack.host.scan_command_state,
            deadline: if config.timeout.as_ticks() == 0 {
                None
            } else {
                Some(Instant::now() + config.timeout.into())
            },
            done: false,
        })
    }

    /// Performs an extended BLE scan, return a report for discovering peripherals.
    ///
    /// Scan is stopped when a report is received -- *Is it really??*. Call this method repeatedly to continue scanning.
    pub async fn scan_ext(&mut self, config: &ScanConfig<'_>) -> Result<ScanSession<'_, true>, BleHostError<C::Error>>
    where
        C: ControllerCmdSync<LeSetExtScanEnable>
            + ControllerCmdSync<LeSetExtScanParams>
            + ControllerCmdSync<LeClearFilterAcceptList>
            + ControllerCmdSync<LeAddDeviceToFilterAcceptList>,
    {
        let host = &self.central.stack.host;
        let drop = crate::host::OnDrop::new(|| {
            host.scan_command_state.cancel(false);
        });
        host.scan_command_state.request().await;
        self.central.set_accept_filter(config.filter_accept_list).await?;

        let scanning = ScanningPhy {
            active_scan: config.active,
            scan_interval: config.interval.into(),
            scan_window: config.window.into(),
        };
        let phy_params = crate::central::create_phy_params(scanning, config.phys);
        let host = &self.central.stack.host;
        host.command(LeSetExtScanParams::new(
            host.address.map(|s| s.kind).unwrap_or(AddrKind::PUBLIC),
            if config.filter_accept_list.is_empty() {
                bt_hci::param::ScanningFilterPolicy::BasicUnfiltered
            } else {
                bt_hci::param::ScanningFilterPolicy::BasicFiltered
            },
            phy_params,
        ))
        .await?;

        host.command(LeSetExtScanEnable::new(
            true,
            FilterDuplicates::Disabled,
            config.timeout.into(),
            bt_hci::param::Duration::from_secs(0),
        ))
        .await?;
        drop.defuse();
        Ok(ScanSession {
            command_state: &self.central.stack.host.scan_command_state,
            deadline: if config.timeout.as_ticks() == 0 {
                None
            } else {
                Some(Instant::now() + config.timeout.into())
            },
            done: false,
        })
    }

    ///Stop extended scanning. Needed as currently the timeout for ScanSession does nothing.
    /// Disabling scanning when it is already disabled has no effect.
    pub fn stop_ext_scan(&mut self) -> Result<(), BleHostError<C::Error>>{
        let host = &self.central.stack.host;
        let drop = crate::host::OnDrop::new(|| {
            host.scan_command_state.cancel(false);
        });
        host.scan_command_state.request().await;
        //The Enable parameter determines whether scanning is enabled or disabled.
        //If it is set to 0x00, the remaining parameters shall be ignored.
        //https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-54/out/en/host-controller-interface/host-controller-interface-functional-specification.html#UUID-bfe8407c-4def-2ded-51dd-e47cf9e8916c:~:text=7.8.65.%20LE%20Set%20Extended%20Scan%20Enable%20command
        host.command(LeSetExtScanEnable::new(
            false,
            FilterDuplicates::Disabled,
            bt_hci::param::Duration::from_secs(0),,
            bt_hci::param::Duration::from_secs(0),
        ))
        .await?;
        drop.defuse();
        Ok(())
    }

    ///Stop scanning. Needed as currently the timeout for ScanSession does nothing.
    /// 
    /// **Note**: Has no impact on extended scanning.
    /// Disabling scanning when it is already disabled has no effect.
    /// 
    /// For more info:
    /// https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-54/out/en/host-controller-interface/host-controller-interface-functional-specification.html#UUID-10327f75-4024-80df-14bc-68fe1e42b9e0:~:text=7.8.11.%20LE%20Set%20Scan%20Enable%20command
    pub fn stop_scan(&mut self) -> Result<(), BleHostError<C::Error>>{
        let host = &self.central.stack.host;
        let drop = crate::host::OnDrop::new(|| {
            host.scan_command_state.cancel(false);
        });
        host.scan_command_state.request().await;
        host.command(LeSetScanEnable::new(
            false,
            FilterDuplicates::Disabled,
        ))
        .await?;
        drop.defuse();
        Ok(())
    }

    /// Performs a BLE scan, return a report for discovering peripherals.
    ///
    /// Scan is stopped when a report is received-- *Is it really??*. Call this method repeatedly to continue scanning.
    pub async fn scan(&mut self, config: &ScanConfig<'_>) -> Result<ScanSession<'_, false>, BleHostError<C::Error>>
    where
        C: ControllerCmdSync<LeSetScanParams>
            + ControllerCmdSync<LeSetScanEnable>
            + ControllerCmdSync<LeClearFilterAcceptList>
            + ControllerCmdSync<LeAddDeviceToFilterAcceptList>,
    {
        let host = &self.central.stack.host;
        let drop = crate::host::OnDrop::new(|| {
            host.scan_command_state.cancel(false);
        });
        host.scan_command_state.request().await;

        self.central.set_accept_filter(config.filter_accept_list).await?;

        let params = LeSetScanParams::new(
            if config.active {
                bt_hci::param::LeScanKind::Active
            } else {
                bt_hci::param::LeScanKind::Passive
            },
            config.interval.into(),
            config.window.into(),
            host.address.map(|a| a.kind).unwrap_or(AddrKind::PUBLIC),
            if config.filter_accept_list.is_empty() {
                bt_hci::param::ScanningFilterPolicy::BasicUnfiltered
            } else {
                bt_hci::param::ScanningFilterPolicy::BasicFiltered
            },
        );
        host.command(params).await?;

        host.command(LeSetScanEnable::new(true, true)).await?;
        drop.defuse();
        Ok(ScanSession {
            command_state: &self.central.stack.host.scan_command_state,
            deadline: if config.timeout.as_ticks() == 0 {
                None
            } else {
                Some(Instant::now() + config.timeout.into())
            },
            done: false,
        })
    }
}

/// Handle to an active advertiser which can accept connections.
pub struct ScanSession<'d, const EXTENDED: bool> {
    command_state: &'d CommandState<bool>,
    deadline: Option<Instant>,
    done: bool,
}

impl<const EXTENDED: bool> Drop for ScanSession<'_, EXTENDED> {
    fn drop(&mut self) {
        self.command_state.cancel(EXTENDED);
    }
}
