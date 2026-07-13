import {
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type PointerEvent as ReactPointerEvent,
} from 'react';
import {
  addUserHeroAvatar,
  avatarReferenceNames,
  avatarClassForId,
  avatarImageStyleForId,
  avatarOptionsForProvider,
  defaultAvatarIdForProvider,
  deleteUserHeroAvatar,
  isUserHeroAvatarId,
  loadUserHeroAvatars,
  normalizeHeroAvatarId,
  refreshUserHeroAvatars,
  type HeroAvatarId,
  type UserHeroAvatar,
} from '../lib/hero-avatars';

type AvatarTab = 'generated' | 'uploaded';
const MAX_SOURCE_AVATAR_BYTES = 8 * 1024 * 1024;
const AVATAR_OUTPUT_SIZE = 512;
const AVATAR_OUTPUT_MAX_BYTES = 420_000;
const AVATAR_CROP_PREVIEW_SIZE = 112;
const AVATAR_MIN_ZOOM = 0.82;
const AVATAR_NEUTRAL_SLIDER_VALUE = 1 / 3;
const AVATAR_MAX_ZOOM = 3;

interface HeroAvatarPickerProps {
  provider: string;
  value?: string | null;
  disabled?: boolean;
  className?: string;
  onChange: (avatarId: HeroAvatarId) => void;
}

interface HeroAvatarArtProps {
  avatarId?: string | null;
  provider?: string | null;
  className?: string;
  labelled?: boolean;
  title?: string;
}

interface PendingUpload {
  fileName: string;
  dataUrl: string;
  width: number;
  height: number;
}

interface CropOffset {
  x: number;
  y: number;
}

export function HeroAvatarArt({
  avatarId,
  provider,
  className = '',
  labelled = false,
  title,
}: HeroAvatarArtProps) {
  return (
    <span
      className={['tavern-avatar-art', avatarClassForId(avatarId, provider), className].filter(Boolean).join(' ')}
      style={avatarImageStyleForId(avatarId)}
      title={title}
      aria-hidden={labelled ? undefined : true}
    >
      <span />
      <i />
      <b />
    </span>
  );
}

export function HeroAvatarPicker({
  provider,
  value,
  disabled = false,
  className = '',
  onChange,
}: HeroAvatarPickerProps) {
  const selected = normalizeHeroAvatarId(value, provider);
  const [open, setOpen] = useState(false);
  const [tab, setTab] = useState<AvatarTab>('generated');
  const [userAvatars, setUserAvatars] = useState<UserHeroAvatar[]>(() => loadUserHeroAvatars());
  const [pendingUpload, setPendingUpload] = useState<PendingUpload | null>(null);
  const [cropZoom, setCropZoom] = useState(1);
  const [cropOffset, setCropOffset] = useState<CropOffset>({ x: 0, y: 0 });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const cropDragRef = useRef<{
    pointerId: number;
    startClientX: number;
    startClientY: number;
    startOffset: CropOffset;
  } | null>(null);

  useEffect(() => {
    const refresh = () => setUserAvatars(loadUserHeroAvatars());
    window.addEventListener('storage', refresh);
    window.addEventListener('kota-v2:user-hero-avatars-changed', refresh);
    void refreshUserHeroAvatars().then(setUserAvatars).catch(() => {});
    return () => {
      window.removeEventListener('storage', refresh);
      window.removeEventListener('kota-v2:user-hero-avatars-changed', refresh);
    };
  }, []);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!wrapRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setOpen(false);
    };
    window.addEventListener('pointerdown', onPointerDown);
    window.addEventListener('keydown', onKeyDown);
    return () => {
      window.removeEventListener('pointerdown', onPointerDown);
      window.removeEventListener('keydown', onKeyDown);
    };
  }, [open]);

  const choose = (avatarId: string) => {
    onChange(avatarId);
    setOpen(false);
  };

  const onUpload = async (file: File | undefined) => {
    if (!file) return;
    setError(null);
    if (!file.type.startsWith('image/')) {
      setError('Choose an image file.');
      return;
    }
    if (file.size > MAX_SOURCE_AVATAR_BYTES) {
      setError('Image is too large. Use an image under 8 MB.');
      return;
    }
    try {
      const dataUrl = await readFileAsDataUrl(file);
      const image = await loadImage(dataUrl);
      const width = image.naturalWidth || image.width;
      const height = image.naturalHeight || image.height;
      if (!width || !height) throw new Error('Unable to read image size.');
      setPendingUpload({ fileName: file.name, dataUrl, width, height });
      setCropZoom(1);
      setCropOffset({ x: 0, y: 0 });
      setTab('uploaded');
      if (inputRef.current) inputRef.current.value = '';
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const savePendingUpload = async () => {
    if (!pendingUpload) return;
    setBusy(true);
    setError(null);
    try {
      const dataUrl = await renderCroppedAvatarDataUrl(pendingUpload.dataUrl, {
        zoom: cropZoom,
        offset: cropOffset,
      });
      const avatar = await addUserHeroAvatar(pendingUpload.fileName, dataUrl);
      setUserAvatars(await refreshUserHeroAvatars());
      setPendingUpload(null);
      setCropZoom(1);
      setCropOffset({ x: 0, y: 0 });
      setTab('uploaded');
      setOpen(false);
      onChange(avatar.id);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const onDelete = async (avatarId: string) => {
    const references = avatarReferenceNames(avatarId, selected === avatarId ? ['current hero'] : []);
    if (references.length > 0) {
      setError(`Used by ${references.join(', ')}. Select another avatar before deleting.`);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await deleteUserHeroAvatar(avatarId);
      const next = await refreshUserHeroAvatars();
      setUserAvatars(next);
      if (selected === avatarId) onChange(defaultAvatarIdForProvider(provider));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const onDeleteClick = (avatarId: string) => {
    void onDelete(avatarId);
  };

  const chooseUploaded = (avatarId: string) => {
    setError(null);
    onChange(avatarId);
    setOpen(false);
  };

  const onCropZoomChange = (nextZoom: number) => {
    const zoom = clamp(nextZoom, AVATAR_MIN_ZOOM, AVATAR_MAX_ZOOM);
    setCropZoom(zoom);
    setCropOffset((offset) => (
      pendingUpload
        ? clampCropOffset(offset, pendingUpload.width, pendingUpload.height, zoom, AVATAR_OUTPUT_SIZE)
        : offset
    ));
  };

  const onCropPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (busy || !pendingUpload) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    cropDragRef.current = {
      pointerId: event.pointerId,
      startClientX: event.clientX,
      startClientY: event.clientY,
      startOffset: cropOffset,
    };
  };

  const onCropPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    const drag = cropDragRef.current;
    if (!drag || drag.pointerId !== event.pointerId || !pendingUpload) return;
    event.preventDefault();
    const outputPixelsPerPreviewPixel = AVATAR_OUTPUT_SIZE / AVATAR_CROP_PREVIEW_SIZE;
    const next = {
      x: drag.startOffset.x + (event.clientX - drag.startClientX) * outputPixelsPerPreviewPixel,
      y: drag.startOffset.y + (event.clientY - drag.startClientY) * outputPixelsPerPreviewPixel,
    };
    setCropOffset(clampCropOffset(
      next,
      pendingUpload.width,
      pendingUpload.height,
      cropZoom,
      AVATAR_OUTPUT_SIZE,
    ));
  };

  const onCropPointerEnd = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (cropDragRef.current?.pointerId === event.pointerId) {
      cropDragRef.current = null;
    }
  };

  const cropPreviewStyle = pendingUpload
    ? cropImageStyle(pendingUpload.width, pendingUpload.height, cropZoom, cropOffset)
    : undefined;
  const cropZoomSliderValue = zoomToSliderValue(cropZoom);

  return (
    <div ref={wrapRef} className="hero-avatar-picker-wrap">
      <button
        type="button"
        className="hero-avatar-trigger"
        disabled={disabled}
        aria-label="Change avatar"
        aria-expanded={open}
        onClick={() => setOpen((next) => !next)}
      >
        <HeroAvatarArt avatarId={selected} provider={provider} className={className} />
        <small>Change</small>
      </button>

      {open && (
        <div className="hero-avatar-popover" role="dialog" aria-label="Choose avatar">
          <div className="hero-avatar-tabs" role="tablist" aria-label="Avatar source">
            <button
              type="button"
              role="tab"
              aria-selected={tab === 'generated'}
              className={tab === 'generated' ? 'active' : ''}
              onClick={() => setTab('generated')}
            >
              Generated
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={tab === 'uploaded'}
              className={tab === 'uploaded' ? 'active' : ''}
              onClick={() => setTab('uploaded')}
            >
              Mine
            </button>
          </div>

          {tab === 'generated' ? (
            <div className="hero-avatar-grid" role="radiogroup" aria-label="Generated avatars">
              {avatarOptionsForProvider(provider).map((option) => (
                <button
                  key={option.id}
                  type="button"
                  className={selected === option.id ? 'active' : ''}
                  role="radio"
                  aria-checked={selected === option.id}
                  title={option.label}
                  onClick={() => choose(option.id)}
                >
                  <HeroAvatarArt avatarId={option.id} provider={provider} />
                  <small>{option.label}</small>
                </button>
              ))}
            </div>
          ) : (
            <div className="hero-avatar-upload-panel">
              <input
                ref={inputRef}
                type="file"
                accept="image/*"
                hidden
                onChange={(event) => void onUpload(event.currentTarget.files?.[0])}
              />
              <button
                type="button"
                className="hero-avatar-upload-button"
                disabled={busy}
                onClick={() => inputRef.current?.click()}
              >
                Upload Image
              </button>
              {error && <div className="hero-avatar-error" role="alert">{error}</div>}
              {pendingUpload && (
                <div className="hero-avatar-cropper" aria-label="Crop uploaded avatar">
                  <div
                    className="hero-avatar-crop-preview"
                    onPointerDown={onCropPointerDown}
                    onPointerMove={onCropPointerMove}
                    onPointerUp={onCropPointerEnd}
                    onPointerCancel={onCropPointerEnd}
                  >
                    <img
                      src={pendingUpload.dataUrl}
                      alt=""
                      draggable={false}
                      style={cropPreviewStyle}
                    />
                  </div>
                  <div className="hero-avatar-crop-fields">
                    <label>
                      <span>Zoom</span>
                      <input
                        type="range"
                        min="0"
                        max="1"
                        step="0.001"
                        value={cropZoomSliderValue}
                        disabled={busy}
                        onChange={(event) => onCropZoomChange(sliderValueToZoom(Number(event.currentTarget.value)))}
                      />
                    </label>
                  </div>
                  <div className="hero-avatar-crop-actions">
                    <button type="button" disabled={busy} onClick={() => void savePendingUpload()}>
                      {busy ? 'Saving' : 'Save Avatar'}
                    </button>
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => {
                        setPendingUpload(null);
                        setCropOffset({ x: 0, y: 0 });
                        setError(null);
                      }}
                    >
                      Cancel
                    </button>
                  </div>
                </div>
              )}
              {userAvatars.length === 0 ? (
                <div className="hero-avatar-empty">No uploaded avatars</div>
              ) : (
                <div className="hero-avatar-grid uploaded" role="radiogroup" aria-label="Uploaded avatars">
                  {userAvatars.map((avatar) => (
                    <span key={avatar.id} className="hero-avatar-user-option">
                      <button
                        type="button"
                        className={selected === avatar.id ? 'active' : ''}
                        role="radio"
                        aria-checked={selected === avatar.id}
                        title={avatar.label}
                        onClick={() => chooseUploaded(avatar.id)}
                      >
                        <HeroAvatarArt avatarId={avatar.id} provider={provider} />
                        <small>{avatar.label}</small>
                      </button>
                      <button
                        type="button"
                        className="hero-avatar-delete"
                        aria-label={`Delete ${avatar.label}`}
                        disabled={busy}
                        onClick={(event) => {
                          event.stopPropagation();
                          onDeleteClick(avatar.id);
                        }}
                      >
                        ×
                      </button>
                    </span>
                  ))}
                </div>
              )}
            </div>
          )}

          {isUserHeroAvatarId(selected) && tab === 'generated' && (
            <button type="button" className="hero-avatar-reset" onClick={() => choose(defaultAvatarIdForProvider(provider))}>
              Reset to default
            </button>
          )}
        </div>
      )}
    </div>
  );
}

function readFileAsDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const result = reader.result;
      if (typeof result === 'string' && result.startsWith('data:image/')) resolve(result);
      else reject(new Error('Unsupported image'));
    };
    reader.onerror = () => reject(reader.error ?? new Error('Unable to read image'));
    reader.readAsDataURL(file);
  });
}

async function renderCroppedAvatarDataUrl(
  sourceDataUrl: string,
  crop: { zoom: number; offset: CropOffset },
): Promise<string> {
  const image = await loadImage(sourceDataUrl);
  const width = image.naturalWidth || image.width;
  const height = image.naturalHeight || image.height;
  if (!width || !height) throw new Error('Unable to read image size.');

  const canvas = document.createElement('canvas');
  canvas.width = AVATAR_OUTPUT_SIZE;
  canvas.height = AVATAR_OUTPUT_SIZE;
  const context = canvas.getContext('2d');
  if (!context) throw new Error('Unable to prepare image crop.');

  const geometry = cropImageGeometry(width, height, crop.zoom, crop.offset, AVATAR_OUTPUT_SIZE);

  context.imageSmoothingEnabled = true;
  context.imageSmoothingQuality = 'high';
  context.drawImage(image, geometry.drawX, geometry.drawY, geometry.drawWidth, geometry.drawHeight);

  const hasTransparentPadding = geometry.drawWidth < AVATAR_OUTPUT_SIZE || geometry.drawHeight < AVATAR_OUTPUT_SIZE;
  const attempts: Array<[string, number]> = hasTransparentPadding ? [
    ['image/webp', 0.9],
    ['image/webp', 0.82],
    ['image/webp', 0.74],
    ['image/png', 1],
  ] : [
    ['image/webp', 0.9],
    ['image/webp', 0.82],
    ['image/webp', 0.74],
    ['image/jpeg', 0.88],
    ['image/jpeg', 0.78],
    ['image/jpeg', 0.68],
  ];
  for (const [mime, quality] of attempts) {
    const dataUrl = await canvasToDataUrl(canvas, mime, quality);
    if (dataUrl.startsWith(`data:${mime};`) && dataUrlByteLength(dataUrl) <= AVATAR_OUTPUT_MAX_BYTES) {
      return dataUrl;
    }
  }
  throw new Error('Cropped image is too large. Zoom in or choose a smaller image.');
}

function loadImage(dataUrl: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const image = new Image();
    image.onload = () => resolve(image);
    image.onerror = () => reject(new Error('Unable to load image.'));
    image.src = dataUrl;
  });
}

function canvasToDataUrl(canvas: HTMLCanvasElement, mime: string, quality: number): Promise<string> {
  return new Promise((resolve, reject) => {
    if (!canvas.toBlob) {
      resolve(canvas.toDataURL(mime, quality));
      return;
    }
    canvas.toBlob((blob) => {
      if (!blob) {
        resolve(canvas.toDataURL(mime, quality));
        return;
      }
      const reader = new FileReader();
      reader.onload = () => {
        if (typeof reader.result === 'string') resolve(reader.result);
        else reject(new Error('Unable to encode image.'));
      };
      reader.onerror = () => reject(reader.error ?? new Error('Unable to encode image.'));
      reader.readAsDataURL(blob);
    }, mime, quality);
  });
}

function dataUrlByteLength(dataUrl: string): number {
  const payload = dataUrl.split(',')[1] ?? '';
  return Math.floor((payload.length * 3) / 4);
}

function cropImageStyle(
  sourceWidth: number,
  sourceHeight: number,
  zoom: number,
  offset: CropOffset,
): CSSProperties {
  const geometry = cropImageGeometry(sourceWidth, sourceHeight, zoom, offset, AVATAR_CROP_PREVIEW_SIZE);
  return {
    width: `${geometry.drawWidth}px`,
    height: `${geometry.drawHeight}px`,
    transform: `translate(${geometry.drawX}px, ${geometry.drawY}px)`,
  };
}

function cropImageGeometry(
  sourceWidth: number,
  sourceHeight: number,
  zoom: number,
  offset: CropOffset,
  outputSize: number,
) {
  const safeZoom = clamp(zoom, AVATAR_MIN_ZOOM, AVATAR_MAX_ZOOM);
  const scale = Math.max(outputSize / sourceWidth, outputSize / sourceHeight) * safeZoom;
  const drawWidth = sourceWidth * scale;
  const drawHeight = sourceHeight * scale;
  const scaledOffset = outputSize === AVATAR_OUTPUT_SIZE
    ? offset
    : {
        x: offset.x * (outputSize / AVATAR_OUTPUT_SIZE),
        y: offset.y * (outputSize / AVATAR_OUTPUT_SIZE),
      };
  const clampedOffset = clampCropOffset(
    scaledOffset,
    sourceWidth,
    sourceHeight,
    safeZoom,
    outputSize,
  );
  return {
    drawWidth,
    drawHeight,
    drawX: (outputSize - drawWidth) / 2 + clampedOffset.x,
    drawY: (outputSize - drawHeight) / 2 + clampedOffset.y,
  };
}

function clampCropOffset(
  offset: CropOffset,
  sourceWidth: number,
  sourceHeight: number,
  zoom: number,
  outputSize: number,
): CropOffset {
  const safeZoom = clamp(zoom, AVATAR_MIN_ZOOM, AVATAR_MAX_ZOOM);
  const scale = Math.max(outputSize / sourceWidth, outputSize / sourceHeight) * safeZoom;
  const maxX = Math.max(0, (sourceWidth * scale - outputSize) / 2);
  const maxY = Math.max(0, (sourceHeight * scale - outputSize) / 2);
  return {
    x: clamp(offset.x, -maxX, maxX),
    y: clamp(offset.y, -maxY, maxY),
  };
}

function sliderValueToZoom(value: number): number {
  const sliderValue = clamp(value, 0, 1);
  if (sliderValue <= AVATAR_NEUTRAL_SLIDER_VALUE) {
    const t = sliderValue / AVATAR_NEUTRAL_SLIDER_VALUE;
    return AVATAR_MIN_ZOOM + (1 - AVATAR_MIN_ZOOM) * t;
  }
  const t = (sliderValue - AVATAR_NEUTRAL_SLIDER_VALUE) / (1 - AVATAR_NEUTRAL_SLIDER_VALUE);
  return 1 + (AVATAR_MAX_ZOOM - 1) * t;
}

function zoomToSliderValue(zoom: number): number {
  const safeZoom = clamp(zoom, AVATAR_MIN_ZOOM, AVATAR_MAX_ZOOM);
  if (safeZoom <= 1) {
    const t = (safeZoom - AVATAR_MIN_ZOOM) / (1 - AVATAR_MIN_ZOOM);
    return AVATAR_NEUTRAL_SLIDER_VALUE * t;
  }
  const t = (safeZoom - 1) / (AVATAR_MAX_ZOOM - 1);
  return AVATAR_NEUTRAL_SLIDER_VALUE + (1 - AVATAR_NEUTRAL_SLIDER_VALUE) * t;
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}
