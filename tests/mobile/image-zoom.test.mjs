import test from 'node:test';
import assert from 'node:assert/strict';
import { constrainImage, MAX_IMAGE_SCALE, zoomImage } from '../../apps/mobile/src/features/conversation/preview/imageZoom.ts';

test('放大后仍能在手指位置看到同一处图像细节', () => {
  const enlarged = zoomImage({ scale: 1, x: 0, y: 0 }, 2, { x: 50, y: -20 });
  assert.equal(enlarged.scale * 50 + enlarged.x, 50);
  assert.equal(enlarged.scale * -20 + enlarged.y, -20);
});

test('双指缩放同时移动时，图像细节跟随双指中心', () => {
  const moved = zoomImage({ scale: 2, x: 10, y: -10 }, 3, { x: 90, y: 30 }, { x: 120, y: 60 });
  assert.equal(moved.scale * 40 + moved.x, 120);
  assert.equal(moved.scale * 20 + moved.y, 60);
});

test('缩放限于适应屏幕到最大倍率之间', () => {
  assert.equal(zoomImage({ scale: 1, x: 0, y: 0 }, 0.2).scale, 1);
  assert.equal(zoomImage({ scale: 1, x: 0, y: 0 }, 100).scale, MAX_IMAGE_SCALE);
});

test('拖动可到达图片边缘，小于视口的方向保持居中', () => {
  const image = { width: 300, height: 200 };
  const viewport = { width: 390, height: 650 };
  assert.deepEqual(constrainImage({ scale: 3, x: 1000, y: -500 }, image, viewport), { scale: 3, x: 255, y: 0 });
  assert.deepEqual(constrainImage({ scale: 3, x: -1000, y: 500 }, image, viewport), { scale: 3, x: -255, y: 0 });
  assert.deepEqual(constrainImage({ scale: 1, x: 200, y: 200 }, image, viewport), { scale: 1, x: 0, y: 0 });
});
