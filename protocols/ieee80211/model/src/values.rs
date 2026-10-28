use super::*;

// Closed, synchronous value operations restore storage before any waker, clock,
// body callback or other external code can run. No API lends a reference into
// a shared cell. In particular a wake may reenter only after every owner is back.
pub(super) struct OwnedQueue<T>(pub(super) Cell<VecDeque<T>>);
impl<T> Default for OwnedQueue<T> {
    fn default() -> Self {
        Self(Cell::new(VecDeque::new()))
    }
}
impl<T> OwnedQueue<T> {
    pub(super) fn with_capacity(capacity: usize) -> Self {
        Self(Cell::new(VecDeque::with_capacity(capacity)))
    }
    pub(super) fn push_back(&self, item: T) {
        let mut values = self.0.take();
        values.push_back(item);
        self.0.set(values);
    }
    pub(super) fn push_front(&self, item: T) {
        let mut values = self.0.take();
        values.push_front(item);
        self.0.set(values);
    }
    pub(super) fn pop_front(&self) -> Option<T> {
        let mut values = self.0.take();
        let item = values.pop_front();
        self.0.set(values);
        item
    }
    pub(super) fn len(&self) -> usize {
        let values = self.0.take();
        let len = values.len();
        self.0.set(values);
        len
    }
    pub(super) fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub(super) fn take_batch(&self) -> VecDeque<T> {
        self.0.take()
    }
    pub(super) fn clear(&self) {
        self.0.take().clear();
    }
}
pub(super) trait Snapshot {
    type Output;
    fn snapshot(&self) -> Self::Output;
}
impl Snapshot for Cell<DriverObservation> {
    type Output = DriverObservation;
    fn snapshot(&self) -> DriverObservation {
        let value = self.take();
        let result = value.clone();
        self.set(value);
        result
    }
}
impl Snapshot for Cell<Option<(Vec<u8>, ModelRxMeta)>> {
    type Output = Option<(Vec<u8>, ModelRxMeta)>;
    fn snapshot(&self) -> Self::Output {
        let value = self.take();
        let result = value.clone();
        self.set(value);
        result
    }
}
impl Snapshot for Cell<Vec<AirFrame>> {
    type Output = Vec<AirFrame>;
    fn snapshot(&self) -> Self::Output {
        let value = self.take();
        let result = value.clone();
        self.set(value);
        result
    }
}
impl Snapshot for Cell<Vec<ModelCryptoOperation>> {
    type Output = Vec<ModelCryptoOperation>;
    fn snapshot(&self) -> Self::Output {
        let value = self.take();
        let result = value.clone();
        self.set(value);
        result
    }
}
pub(super) trait Record<T> {
    fn record(&self, item: T);
    fn record_all(&self, items: Vec<T>);
}
// Inserting owned elements into Vec calls no element code.
impl<T> Record<T> for Cell<Vec<T>> {
    fn record(&self, item: T) {
        let mut values = self.take();
        values.push(item);
        self.set(values);
    }
    fn record_all(&self, items: Vec<T>) {
        let mut values = self.take();
        values.extend(items);
        self.set(values);
    }
}
pub(super) trait ModelSettingsCell {
    fn mark_other(&self);
    fn set_block_ack(&self, slot: u8, wanted: Option<ModelReceiveAgreement>);
}
impl ModelSettingsCell for Cell<Settings> {
    fn mark_other(&self) {
        let mut value = self.take();
        value.other = true;
        self.set(value);
    }
    fn set_block_ack(&self, slot: u8, wanted: Option<ModelReceiveAgreement>) {
        let mut value = self.take();
        value.block_acks.insert(slot, wanted);
        self.set(value);
    }
}
pub(super) trait AssociationsCell {
    fn apply(&self, role: VifRole, spp: oer_ieee80211_wire::security::Spp);
}
impl AssociationsCell for Cell<InterfaceAssociations> {
    fn apply(&self, role: VifRole, spp: oer_ieee80211_wire::security::Spp) {
        let mut value = self.get();
        value.apply(role, spp);
        self.set(value);
    }
}
pub(super) trait EdcaCell {
    fn parameters(&self, role: VifRole) -> EdcaParameterSet;
    fn set_parameters(&self, role: VifRole, parameters: EdcaParameterSet);
    fn contentions(&self) -> Vec<ModelContention>;
    fn set_entropy(&self, entropy: u32);
    fn reset(&self, ac: WmmAccessCategory);
    fn failure(&self, ac: WmmAccessCategory);
    fn draw(&self, role: VifRole, ac: WmmAccessCategory);
}
impl EdcaCell for Cell<ModelEdca> {
    fn parameters(&self, role: VifRole) -> EdcaParameterSet {
        let value = self.take();
        let result = value.parameters(role);
        self.set(value);
        result
    }
    fn set_parameters(&self, role: VifRole, parameters: EdcaParameterSet) {
        let mut value = self.take();
        *value.parameters_mut(role) = parameters;
        self.set(value);
    }
    fn contentions(&self) -> Vec<ModelContention> {
        let value = self.take();
        let result = value.draws.clone();
        self.set(value);
        result
    }
    fn set_entropy(&self, entropy: u32) {
        let mut value = self.take();
        value.entropy = entropy;
        self.set(value);
    }
    fn reset(&self, ac: WmmAccessCategory) {
        let mut value = self.take();
        value.reset(ac);
        self.set(value);
    }
    fn failure(&self, ac: WmmAccessCategory) {
        let mut value = self.take();
        value.failure(ac);
        self.set(value);
    }
    fn draw(&self, role: VifRole, ac: WmmAccessCategory) {
        let mut value = self.take();
        value.draw(role, ac);
        self.set(value);
    }
}
