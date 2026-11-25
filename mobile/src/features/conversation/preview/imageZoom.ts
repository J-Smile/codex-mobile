export type ImagePoint = { x: number; y: number };
export type ImageTransform = ImagePoint & { scale: number };
type ImageSize = { width: number; height: number };

export const MAX_IMAGE_SCALE = 8;

// 缩放时保持手指下的图像位置不变，同时跟随双指中心移动。
export function zoomImage(current: ImageTransform, nextScale: number, from: ImagePoint = { x: 0, y: 0 }, to = from): ImageTransform {
  const scale = Math.max(1, Math.min(MAX_IMAGE_SCALE, nextScale));
  const ratio = scale / current.scale;
  return { scale, x: to.x - (from.x - current.x) * ratio, y: to.y - (from.y - current.y) * ratio };
}

export function constrainImage(current: ImageTransform, image: ImageSize, viewport: ImageSize): ImageTransform {
  const limitX = Math.max(0, (image.width * current.scale - viewport.width) / 2);
  const limitY = Math.max(0, (image.height * current.scale - viewport.height) / 2);
  return {
    ...current,
    x: limitX === 0 ? 0 : Math.max(-limitX, Math.min(limitX, current.x)),
    y: limitY === 0 ? 0 : Math.max(-limitY, Math.min(limitY, current.y)),
  };
}
