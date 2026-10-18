import '../styles/local-model-credit.css';

export const localModelProvider = (model: string) => {
  const separator = model.indexOf('/');
  return separator < 0 ? '' : model.slice(0, separator);
};
export const usesLocalCredit = (providerId?: string) => providerId === 'anthropic';

/** A display label only; it neither probes credentials nor changes selection. */
export function LocalModelCredit({ providerId }: { providerId?: string }) {
  return usesLocalCredit(providerId) ? <span className="local-model-credit">(Use Credit)</span> : null;
}

export const localModelOptionCredit = (option: { id: string }) =>
  <LocalModelCredit providerId={localModelProvider(option.id)} />;
